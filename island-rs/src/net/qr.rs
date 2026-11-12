//! Small QR encoder: byte mode, error correction level L, versions 1-5
//! (up to 106 bytes, single Reed-Solomon block). Enough for a local share URL.

// Index = version. Total codewords and EC codewords for level L.
const TOTAL_CODEWORDS: [usize; 6] = [0, 26, 44, 70, 100, 134];
const ECC_CODEWORDS: [usize; 6] = [0, 7, 10, 15, 20, 26];
const MAX_VERSION: usize = 5;
const MASK: u32 = 0; // (x + y) % 2 == 0

pub struct Qr {
    pub size: usize,
    modules: Vec<bool>,
}

impl Qr {
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

struct Grid {
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}

impl Grid {
    fn set_function(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }
}

/// Returns the module matrix, or None if the text is too long.
pub fn encode(text: &str) -> Option<Qr> {
    let payload = text.as_bytes();
    let version = (1..=MAX_VERSION).find(|&v| payload.len() + 2 <= TOTAL_CODEWORDS[v] - ECC_CODEWORDS[v])?;

    let data_len = TOTAL_CODEWORDS[version] - ECC_CODEWORDS[version];
    let mut all = build_data_codewords(payload, data_len);
    let ecc = reed_solomon_remainder(&all, &reed_solomon_divisor(ECC_CODEWORDS[version]));
    all.extend_from_slice(&ecc);

    let size = version * 4 + 17;
    let mut grid = Grid { size, modules: vec![false; size * size], function: vec![false; size * size] };

    draw_function_patterns(&mut grid, version);
    draw_codewords(&mut grid, &all);

    for y in 0..size {
        for x in 0..size {
            if !grid.function[y * size + x] && (x + y) % 2 == 0 {
                grid.modules[y * size + x] = !grid.modules[y * size + x];
            }
        }
    }

    draw_format_bits(&mut grid);
    Some(Qr { size, modules: grid.modules })
}

fn build_data_codewords(payload: &[u8], data_len: usize) -> Vec<u8> {
    let mut result = vec![0u8; data_len];
    let mut bit_pos = 0usize;

    append_bits(&mut result, &mut bit_pos, 0x4, 4); // byte mode
    append_bits(&mut result, &mut bit_pos, payload.len() as u32, 8); // character count (versions 1-9)
    for &b in payload {
        append_bits(&mut result, &mut bit_pos, b as u32, 8);
    }

    let capacity_bits = data_len * 8;
    let terminator = 4.min(capacity_bits - bit_pos);
    append_bits(&mut result, &mut bit_pos, 0, terminator);
    let align = (8 - bit_pos % 8) % 8;
    append_bits(&mut result, &mut bit_pos, 0, align);

    let mut pad = 0xECu32;
    while bit_pos < capacity_bits {
        append_bits(&mut result, &mut bit_pos, pad, 8);
        pad ^= 0xEC ^ 0x11;
    }
    result
}

fn append_bits(target: &mut [u8], bit_pos: &mut usize, value: u32, count: usize) {
    for i in (0..count).rev() {
        if (value >> i) & 1 != 0 {
            target[*bit_pos >> 3] |= 0x80 >> (*bit_pos & 7);
        }
        *bit_pos += 1;
    }
}

fn draw_function_patterns(grid: &mut Grid, version: usize) {
    let size = grid.size;

    // Timing patterns
    for i in 0..size {
        grid.set_function(6, i, i % 2 == 0);
        grid.set_function(i, 6, i % 2 == 0);
    }

    // Finder patterns (with separators)
    draw_finder(grid, 3, 3);
    draw_finder(grid, size as i32 - 4, 3);
    draw_finder(grid, 3, size as i32 - 4);

    // Versions 2-6 have exactly one alignment pattern, bottom right.
    if version >= 2 {
        let c = size as i32 - 7;
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                grid.set_function((c + dx) as usize, (c + dy) as usize, dx.abs().max(dy.abs()) != 1);
            }
        }
    }

    // Reserve the format areas; real bits are written after masking.
    draw_format_bits(grid);
}

fn draw_finder(grid: &mut Grid, cx: i32, cy: i32) {
    let size = grid.size as i32;
    for dy in -4i32..=4 {
        for dx in -4i32..=4 {
            let (x, y) = (cx + dx, cy + dy);
            if x < 0 || x >= size || y < 0 || y >= size {
                continue;
            }
            let dist = dx.abs().max(dy.abs());
            grid.set_function(x as usize, y as usize, dist != 2 && dist != 4);
        }
    }
}

fn bit(value: u32, index: usize) -> bool {
    (value >> index) & 1 != 0
}

fn draw_format_bits(grid: &mut Grid) {
    let size = grid.size;
    let data = (1 << 3) | MASK; // level L = 01
    let mut rem = data;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    let bits = ((data << 10) | rem) ^ 0x5412;

    for i in 0..=5 {
        grid.set_function(8, i, bit(bits, i));
    }
    grid.set_function(8, 7, bit(bits, 6));
    grid.set_function(8, 8, bit(bits, 7));
    grid.set_function(7, 8, bit(bits, 8));
    for i in 9..15 {
        grid.set_function(14 - i, 8, bit(bits, i));
    }

    for i in 0..8 {
        grid.set_function(size - 1 - i, 8, bit(bits, i));
    }
    for i in 8..15 {
        grid.set_function(8, size - 15 + i, bit(bits, i));
    }
    grid.set_function(8, size - 8, true); // always dark
}

fn draw_codewords(grid: &mut Grid, data: &[u8]) {
    let size = grid.size as i32;
    let mut i = 0usize;
    let mut right = size - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..size {
            for j in 0..2 {
                let x = right - j;
                let upward = ((right + 1) & 2) == 0;
                let y = if upward { size - 1 - vert } else { vert };
                let index = (y * size + x) as usize;
                if !grid.function[index] && i < data.len() * 8 {
                    grid.modules[index] = (data[i >> 3] >> (7 - (i & 7))) & 1 != 0;
                    i += 1;
                }
            }
        }
        right -= 2;
    }
}

fn reed_solomon_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u32;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = multiply(result[j] as u32, root) as u8;
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = multiply(root, 2);
    }
    result
}

fn reed_solomon_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = (b ^ result[0]) as u32;
        result.rotate_left(1);
        let last = result.len() - 1;
        result[last] = 0;
        for i in 0..result.len() {
            result[i] ^= multiply(divisor[i] as u32, factor) as u8;
        }
    }
    result
}

fn multiply(x: u32, y: u32) -> u32 {
    let mut z = 0u32;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((y >> i) & 1) * x;
    }
    z & 0xFF
}
