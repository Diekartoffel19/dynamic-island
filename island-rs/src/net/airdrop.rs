//! Nearby share: send files to other PCs on the local network that run the island.
//!
//! Discovery, UDP port 19847. Traffic only flows while a picker is open:
//!   "ISLAND1 PROBE <id>"               broadcast by the sender while its picker is open
//!   "ISLAND1 HERE <id> <port> <name>"  answer of every visible island (also sent once when it turns visible)
//!   "ISLAND1 BYE <id>"                 broadcast when an island turns invisible or quits
//!
//! Transfer, TCP port 19847:
//!   sender:   "ISLAND1 OFFER <id>\n" "FROM <name>\n" ("FILE <size> <name>\n")* "\n"
//!   receiver: "OK\n" once the user accepts on the island, else "NO\n"
//!   sender:   the file bytes back to back, in the announced order and sizes
//!   receiver: "DONE\n" after the last file is on disk
//!
//! Names are percent-encoded. Nothing is written to disk before the user accepts.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::share_server::{create_token, default_receive_folder, file_name, free_space, sanitize_file_name, unique_path};
use super::{url_decode, url_encode};
use crate::event::{post, EventType, IslandEvent, Msg};
use crate::sys::network;

pub const PORT: u16 = 19847;
const MAGIC: &str = "ISLAND1";
const MAX_FILES: usize = 1000;
const MAX_LINE: u64 = 1024;
/// How long an offer waits for the user. The island declines on its own after this.
pub const DECIDE_SECONDS: u64 = 60;
/// Offers handled at the same time; more connections are closed right away.
const MAX_ACTIVE: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SendOutcome {
    Sent,
    Declined,
    Failed,
}

struct Node {
    id: String,
    name: String,
    udp: UdpSocket,
}

static NODE: OnceLock<Option<Node>> = OnceLock::new();
static VISIBLE: AtomicBool = AtomicBool::new(false);
static TCP_STARTED: AtomicBool = AtomicBool::new(false);
static NEXT_OFFER: AtomicU32 = AtomicU32::new(1);
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static PENDING: Mutex<Vec<(u32, Sender<bool>)>> = Mutex::new(Vec::new());

/// The name other PCs see.
fn computer_name() -> String {
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    if name.is_empty() {
        "Windows-PC".to_string()
    } else {
        name
    }
}

/// Binds the UDP socket and starts its receive thread on first use.
fn node() -> Option<&'static Node> {
    NODE.get_or_init(|| {
        let udp = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT)).ok()?;
        udp.set_broadcast(true).ok()?;
        let reader = udp.try_clone().ok()?;
        std::thread::Builder::new().name("IslandNearbyUdp".into()).stack_size(64 * 1024).spawn(move || udp_loop(reader)).ok()?;
        Some(Node { id: create_token(6), name: computer_name(), udp })
    })
    .as_ref()
}

fn broadcast(text: &str) {
    let Some(node) = node() else { return };
    for address in network::broadcast_addresses() {
        let _ = node.udp.send_to(text.as_bytes(), SocketAddrV4::new(address, PORT));
    }
}

fn here_line(node: &Node) -> String {
    format!("{MAGIC} HERE {} {PORT} {}", node.id, url_encode(&node.name))
}

/// Visible islands answer probes and accept offers (after asking the user).
pub fn set_visible(visible: bool) {
    let was = VISIBLE.swap(visible, Ordering::SeqCst);
    if visible {
        let Some(node) = node() else { return };
        start_tcp();
        if !was {
            // Pickers that are open right now see this PC at once.
            broadcast(&here_line(node));
        }
    } else if was {
        if let Some(node) = node() {
            broadcast(&format!("{MAGIC} BYE {}", node.id));
        }
    }
}

/// Asks every visible island on the network to say where it is.
/// False if the discovery port could not be opened.
pub fn probe() -> bool {
    let Some(node) = node() else { return false };
    broadcast(&format!("{MAGIC} PROBE {}", node.id));
    true
}

/// Tells the network this island is gone.
pub fn shutdown() {
    if VISIBLE.load(Ordering::SeqCst) {
        set_visible(false);
    }
}

fn udp_loop(udp: UdpSocket) {
    let mut buffer = [0u8; 600];
    loop {
        // Errors here are one-off (e.g. an ICMP "port unreachable" from an earlier send).
        let Ok((count, source)) = udp.recv_from(&mut buffer) else { continue };
        let Ok(text) = std::str::from_utf8(&buffer[..count]) else { continue };
        let Some(node) = NODE.get().and_then(|n| n.as_ref()) else { continue };
        let parts: Vec<&str> = text.trim().split(' ').collect();
        if parts.len() < 3 || parts[0] != MAGIC || parts[2] == node.id {
            continue;
        }
        match (parts[1], parts.len()) {
            ("PROBE", _) => {
                if VISIBLE.load(Ordering::SeqCst) {
                    let _ = udp.send_to(here_line(node).as_bytes(), source);
                }
            }
            ("HERE", 5) => {
                let Ok(port) = parts[3].parse::<u16>() else { continue };
                let name = url_decode(parts[4]).map(|n| clean_name(&n)).filter(|n| !n.is_empty()).unwrap_or_else(|| source.ip().to_string());
                post(Msg::PeerFound { id: parts[2].to_string(), name, addr: SocketAddr::new(source.ip(), port) });
            }
            ("BYE", _) => post(Msg::PeerGone { id: parts[2].to_string() }),
            _ => {}
        }
    }
}

/// Printable and short: the name ends up on screen.
fn clean_name(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string()
}

fn read_line(reader: &mut impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    let count = Read::by_ref(reader).take(MAX_LINE).read_line(&mut line)?;
    if count == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if !line.ends_with('\n') {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Copies exactly `size` bytes. `progress` gets the number of bytes of each chunk.
fn copy_exact(input: &mut impl Read, output: &mut impl Write, size: u64, buffer: &mut [u8], progress: &mut dyn FnMut(u64)) -> io::Result<()> {
    let mut remaining = size;
    while remaining > 0 {
        let want = (buffer.len() as u64).min(remaining) as usize;
        let read = input.read(&mut buffer[..want])?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        output.write_all(&buffer[..read])?;
        remaining -= read as u64;
        progress(read as u64);
    }
    Ok(())
}

/// Posts a progress message at most every 120 ms, and always at 100 %.
struct Progress {
    total: u64,
    done: u64,
    last_percent: i32,
    last_post: Instant,
}

impl Progress {
    fn new(total: u64) -> Self {
        Progress { total: total.max(1), done: 0, last_percent: -1, last_post: Instant::now() }
    }

    fn add(&mut self, bytes: u64) -> Option<i32> {
        self.done += bytes;
        let percent = (self.done.saturating_mul(100) / self.total).min(100) as i32;
        if percent != self.last_percent && (percent == 100 || self.last_post.elapsed() >= Duration::from_millis(120)) {
            self.last_percent = percent;
            self.last_post = Instant::now();
            return Some(percent);
        }
        None
    }
}

// =====================================================================
//  Receiving
// =====================================================================

fn start_tcp() {
    if TCP_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(listener) = TcpListener::bind((Ipv4Addr::UNSPECIFIED, PORT)) else {
        TCP_STARTED.store(false, Ordering::SeqCst);
        return;
    };
    let _ = std::thread::Builder::new().name("IslandNearbyTcp".into()).stack_size(64 * 1024).spawn(move || {
        for stream in listener.incoming().flatten() {
            if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_ACTIVE {
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let spawned = std::thread::Builder::new().stack_size(256 * 1024).spawn(move || {
                let _ = receive(stream);
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
            });
            if spawned.is_err() {
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
            }
        }
    });
}

/// The user's answer to an offer shown on the island.
pub fn answer(offer: u32, accept: bool) {
    let mut pending = PENDING.lock().unwrap();
    if let Some(index) = pending.iter().position(|(id, _)| *id == offer) {
        let (_, tx) = pending.remove(index);
        let _ = tx.send(accept);
    }
}

fn receive(stream: TcpStream) -> io::Result<()> {
    let peer_ip = stream.peer_addr().map(|a| a.ip().to_string()).unwrap_or_default();
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::with_capacity(64 * 1024, stream);

    let first = read_line(&mut reader)?;
    let mut head = first.split(' ');
    if head.next() != Some(MAGIC) || head.next() != Some("OFFER") {
        return Ok(());
    }

    let mut from = String::new();
    let mut files: Vec<(u64, String)> = Vec::new();
    loop {
        let line = read_line(&mut reader)?;
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("FROM ") {
            from = url_decode(rest).map(|n| clean_name(&n)).unwrap_or_default();
        } else if let Some(rest) = line.strip_prefix("FILE ") {
            let (size, name) = rest.split_once(' ').ok_or(io::ErrorKind::InvalidData)?;
            let size: u64 = size.parse().map_err(|_| io::ErrorKind::InvalidData)?;
            let name = sanitize_file_name(&url_decode(name).ok_or(io::ErrorKind::InvalidData)?);
            files.push((size, name));
            if files.len() > MAX_FILES {
                return Err(io::ErrorKind::InvalidData.into());
            }
        }
    }
    if from.is_empty() {
        from = peer_ip;
    }
    let total = files.iter().try_fold(0u64, |sum, (size, _)| sum.checked_add(*size)).ok_or(io::ErrorKind::InvalidData)?;
    if files.is_empty() || !VISIBLE.load(Ordering::SeqCst) {
        writer.write_all(b"NO\n")?;
        return Ok(());
    }

    // Ask the user on the island and wait for the answer.
    let offer = NEXT_OFFER.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = channel();
    PENDING.lock().unwrap().push((offer, tx));
    post(Msg::DropOffer { offer, from: from.clone(), names: files.iter().map(|(_, n)| n.clone()).collect(), total });
    let accepted = rx.recv_timeout(Duration::from_secs(DECIDE_SECONDS)).unwrap_or(false);
    PENDING.lock().unwrap().retain(|(id, _)| *id != offer);
    if !accepted {
        post(Msg::DropClosed { offer });
        writer.write_all(b"NO\n")?;
        return Ok(());
    }

    let folder = default_receive_folder();
    fs::create_dir_all(&folder)?;
    if free_space(&folder).is_some_and(|free| free < total) {
        writer.write_all(b"NO\n")?;
        post(Msg::Alert(IslandEvent::new("Empfang abgebrochen", "Nicht genug Speicherplatz", EventType::Error, None, "", 3200)));
        return Ok(());
    }
    writer.write_all(b"OK\n")?;
    reader.get_ref().set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut progress = Progress::new(total);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut paths = Vec::new();
    for (size, name) in files {
        let temp = folder.join(format!(".{}.part", create_token(6)));
        let result = (|| -> io::Result<PathBuf> {
            {
                let mut output = fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
                copy_exact(&mut reader, &mut output, size, &mut buffer, &mut |bytes| {
                    if let Some(percent) = progress.add(bytes) {
                        post(Msg::DropProgress { from: from.clone(), percent });
                    }
                })?;
            }
            let final_path = unique_path(&folder, &name);
            fs::rename(&temp, &final_path)?;
            Ok(final_path)
        })();
        match result {
            Ok(path) => paths.push(path),
            Err(e) => {
                let _ = fs::remove_file(&temp);
                post(Msg::DropDone { from, paths, complete: false });
                return Err(e);
            }
        }
    }
    let _ = writer.write_all(b"DONE\n");
    post(Msg::DropDone { from, paths, complete: true });
    Ok(())
}

// =====================================================================
//  Sending
// =====================================================================

/// Offers the files to the island at `addr` and streams them if it accepts.
/// Folders are skipped. Reports through SendProgress / SendDone.
pub fn send(addr: SocketAddr, to: String, paths: Vec<String>) {
    let _ = std::thread::Builder::new().name("IslandNearbySend".into()).stack_size(256 * 1024).spawn(move || {
        let outcome = send_blocking(addr, &to, &paths).unwrap_or(SendOutcome::Failed);
        post(Msg::SendDone { to, outcome });
    });
}

fn send_blocking(addr: SocketAddr, to: &str, paths: &[String]) -> io::Result<SendOutcome> {
    let Some(node) = node() else { return Ok(SendOutcome::Failed) };
    let files: Vec<(&String, u64)> = paths.iter().filter_map(|p| fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| (p, m.len()))).collect();
    if files.is_empty() {
        return Ok(SendOutcome::Failed);
    }

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    // The other side waits up to a minute for its user.
    stream.set_read_timeout(Some(Duration::from_secs(DECIDE_SECONDS + 15)))?;

    let mut header = format!("{MAGIC} OFFER {}\nFROM {}\n", node.id, url_encode(&node.name));
    for (path, size) in &files {
        header.push_str(&format!("FILE {size} {}\n", url_encode(file_name(path))));
    }
    header.push('\n');
    stream.write_all(header.as_bytes())?;

    let mut reader = BufReader::with_capacity(64, stream.try_clone()?);
    if read_line(&mut reader)? != "OK" {
        return Ok(SendOutcome::Declined);
    }

    let total: u64 = files.iter().map(|(_, size)| *size).sum();
    let mut progress = Progress::new(total);
    let mut buffer = vec![0u8; 64 * 1024];
    for (path, size) in &files {
        // Share mode: read, write and delete, so the file stays usable while it is sent.
        let mut file = fs::OpenOptions::new().read(true).share_mode(7).open(path)?;
        copy_exact(&mut file, &mut stream, *size, &mut buffer, &mut |bytes| {
            if let Some(percent) = progress.add(bytes) {
                post(Msg::SendProgress { to: to.to_string(), percent });
            }
        })?;
    }
    stream.flush()?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    Ok(if read_line(&mut reader)? == "DONE" { SendOutcome::Sent } else { SendOutcome::Failed })
}
