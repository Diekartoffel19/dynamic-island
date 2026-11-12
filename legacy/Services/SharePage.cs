namespace DynamicIsland.Services
{
    /// <summary>The web page a phone sees after scanning the share QR code.</summary>
    internal static class SharePage
    {
        public const string Html = @"<!doctype html>
<html lang='de'>
<head>
<meta charset='utf-8'>
<meta name='viewport' content='width=device-width,initial-scale=1'>
<meta name='referrer' content='no-referrer'>
<title>Dynamic Island</title>
<style>
*{box-sizing:border-box}
body{margin:0;background:#000;color:#fff;font:15px/1.4 -apple-system,'Segoe UI',Roboto,sans-serif;-webkit-text-size-adjust:100%}
main{max-width:560px;margin:0 auto;padding:22px 16px 48px}
header{text-align:center;margin-bottom:26px}
.pill{width:120px;height:30px;border-radius:15px;background:#111;border:1px solid rgba(255,255,255,.12);margin:0 auto 14px}
h1{font-size:20px;margin:0}
h2{font-size:12px;font-weight:600;letter-spacing:.06em;text-transform:uppercase;color:rgba(255,255,255,.6);margin:26px 2px 8px}
.muted{color:rgba(255,255,255,.5);font-size:13px;margin:4px 2px}
ul{list-style:none;margin:0;padding:0}
li{position:relative;overflow:hidden;background:rgba(255,255,255,.08);border-radius:14px;margin-bottom:6px}
li a,li.row{display:flex;align-items:center;gap:12px;padding:13px 14px;color:#fff;text-decoration:none}
li span{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
li small{color:rgba(255,255,255,.55);font-size:12px;flex:none}
li i{position:absolute;left:0;bottom:0;height:3px;width:0;background:linear-gradient(90deg,#4c9bff,#8b6cff);transition:width .2s}
li.ok small{color:#30d158}
li.err small{color:#ff453a}
#drop{display:block;text-align:center;padding:24px 12px;border:1px dashed rgba(255,255,255,.3);border-radius:16px;background:rgba(255,255,255,.03);margin-bottom:8px;cursor:pointer}
#drop.over{border-color:#5aa2ff;background:rgba(90,162,255,.12)}
#drop b{display:block;font-size:15px}
#drop span{color:rgba(255,255,255,.5);font-size:13px}
textarea{width:100%;background:rgba(255,255,255,.08);color:#fff;border:0;border-radius:14px;padding:12px 14px;font:inherit;resize:vertical;outline:none}
button{margin-top:8px;border:0;border-radius:15px;height:32px;padding:0 18px;color:#fff;font:600 13px inherit;background:linear-gradient(135deg,#4c9bff,#8b6cff);cursor:pointer}
#sent{margin-left:10px}
</style>
</head>
<body>
<main>
<header><div class='pill'></div><h1>Dynamic Island</h1><p class='muted'>Dateien mit dem PC austauschen</p></header>

<h2>Vom PC</h2>
<ul id='files'></ul>
<p id='empty' class='muted'>Die Ablage ist leer.</p>

<h2>An den PC senden</h2>
<label id='drop'><input id='pick' type='file' multiple hidden><b>Dateien ausw&auml;hlen</b><span>oder hierher ziehen</span></label>
<ul id='ups'></ul>

<h2>Text an den PC</h2>
<textarea id='txt' rows='3' placeholder='Landet in der Zwischenablage des PCs'></textarea>
<button id='send'>Senden</button><span id='sent' class='muted'></span>
</main>
<script>
var $=function(i){return document.getElementById(i)};
function size(n){var u=['B','KB','MB','GB'],i=0;while(n>=1024&&i<3){n/=1024;i++}return (i?n.toFixed(1):n)+' '+u[i]}
var last=null;
function load(){
  var x=new XMLHttpRequest();x.open('GET','list');
  x.onload=function(){
    if(x.status!==200||x.responseText===last)return;
    last=x.responseText;
    var a=JSON.parse(last),ul=$('files');ul.innerHTML='';
    a.forEach(function(f){
      var li=document.createElement('li'),l=document.createElement('a'),n=document.createElement('span'),s=document.createElement('small');
      l.href=f.url;l.setAttribute('download',f.name);
      n.textContent=f.name;s.textContent=size(f.size);
      l.appendChild(n);l.appendChild(s);li.appendChild(l);ul.appendChild(li);
    });
    $('empty').style.display=a.length?'none':'block';
  };
  x.send();
}
load();setInterval(load,3000);
function up(file){
  var li=document.createElement('li'),n=document.createElement('span'),s=document.createElement('small'),bar=document.createElement('i');
  li.className='row';n.textContent=file.name;s.textContent='0 %';
  li.appendChild(n);li.appendChild(s);li.appendChild(bar);$('ups').appendChild(li);
  var x=new XMLHttpRequest();x.open('POST','upload?name='+encodeURIComponent(file.name));
  x.upload.onprogress=function(e){if(e.lengthComputable&&e.total){var p=Math.round(e.loaded*100/e.total);s.textContent=p+' %';bar.style.width=p+'%'}};
  x.onload=function(){var ok=x.status===200;s.textContent=ok?'Fertig':'Fehler';li.className='row '+(ok?'ok':'err');bar.style.width='100%'};
  x.onerror=function(){s.textContent='Fehler';li.className='row err'};
  x.send(file);
}
function many(l){for(var i=0;i<l.length;i++)up(l[i])}
$('pick').onchange=function(){many(this.files);this.value=''};
var d=$('drop');
d.ondragover=function(e){e.preventDefault();d.className='over'};
d.ondragleave=function(){d.className=''};
d.ondrop=function(e){e.preventDefault();d.className='';many(e.dataTransfer.files)};
$('send').onclick=function(){
  var t=$('txt').value;if(!t)return;
  var x=new XMLHttpRequest();x.open('POST','text');
  x.onload=function(){var ok=x.status===200;$('sent').textContent=ok?'In der Zwischenablage des PCs':'Fehler';if(ok)$('txt').value=''};
  x.onerror=function(){$('sent').textContent='Fehler'};
  x.send(t);
};
</script>
</body>
</html>";
    }
}
