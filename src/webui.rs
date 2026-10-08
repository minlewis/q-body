//! Web UI — 单页聊天界面（浏览器打开 / 即可与 q-body 对话）
//!
//! 纯静态 HTML（无构建、无外部依赖），前端直接 POST /a2a/jsonrpc。
//! 放在独立常量里，不污染 main.rs；TAO：HTML 是呈现层不影响 Rust 逻辑上限。

pub const CHAT_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>q-body chat</title>
<style>
  :root { color-scheme: light dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0; font-family: -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif;
    background: #0f1117; color: #e6e8ee; height: 100vh; display: flex; flex-direction: column;
  }
  header { padding: 14px 18px; border-bottom: 1px solid #232735; font-size: 15px; }
  header .dot { color: #6ee7b7; }
  #log { flex: 1; overflow-y: auto; padding: 18px; display: flex; flex-direction: column; gap: 12px; }
  .msg { max-width: 78%; padding: 10px 14px; border-radius: 14px; line-height: 1.55; white-space: pre-wrap; word-break: break-word; }
  .user { align-self: flex-end; background: #2563eb; color: #fff; border-bottom-right-radius: 4px; }
  .bot { align-self: flex-start; background: #1c2030; border-bottom-left-radius: 4px; }
  .bot.err { background: #3a1d24; color: #fca5a5; }
  form { display: flex; gap: 10px; padding: 14px 18px; border-top: 1px solid #232735; }
  input {
    flex: 1; padding: 12px 14px; border-radius: 12px; border: 1px solid #2c3142;
    background: #171a24; color: #e6e8ee; font-size: 15px; outline: none;
  }
  input:focus { border-color: #2563eb; }
  button {
    padding: 0 20px; border-radius: 12px; border: none; background: #2563eb;
    color: #fff; font-size: 15px; cursor: pointer;
  }
  button:disabled { opacity: .5; cursor: default; }
</style>
</head>
<body>
<header><span class="dot">●</span> q-body <small style="color:#8a90a4">自进化 A2A agent</small></header>
<div id="log"></div>
<form id="f">
  <input id="inp" autocomplete="off" placeholder="给 q-body 发消息…" autofocus>
  <button id="btn">发送</button>
</form>
<script>
const log = document.getElementById('log');
const inp = document.getElementById('inp');
const btn = document.getElementById('btn');

function add(cls, text) {
  const d = document.createElement('div');
  d.className = 'msg ' + cls;
  d.textContent = text;
  log.appendChild(d);
  log.scrollTop = log.scrollHeight;
  return d;
}

async function send(text) {
  add('user', text);
  btn.disabled = true;
  const pending = add('bot', '…');
  try {
    const r = await fetch('/a2a/jsonrpc', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        jsonrpc: '2.0', id: Date.now(), method: 'message/send',
        params: { id: 'web-' + Date.now(), message: { role: 'user', parts: [{ kind: 'text', text }] } }
      })
    });
    const data = await r.json();
    const reply = data?.result?.artifacts?.[0]?.parts?.[0]?.text
      || data?.error?.message || '(无回复)';
    pending.textContent = reply;
    if (!data.result) pending.classList.add('err');
  } catch (e) {
    pending.textContent = '请求失败: ' + e;
    pending.classList.add('err');
  }
  btn.disabled = false;
}

document.getElementById('f').addEventListener('submit', (e) => {
  e.preventDefault();
  const t = inp.value.trim();
  if (!t) return;
  inp.value = '';
  send(t);
  inp.focus();
});
</script>
</body>
</html>
"#;
