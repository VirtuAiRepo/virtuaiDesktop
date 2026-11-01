// Folder tree and the login shell beside the chat. The server's page
// never gets these commands.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const open = new Set();

function row(entry) {
  const item = document.createElement('li');
  const line = document.createElement('button');
  line.type = 'button';
  line.className = 'entry';
  line.textContent = entry.dir ? `${entry.name}/` : entry.name;
  item.appendChild(line);
  if (!entry.dir) return item;
  const child = document.createElement('ul');
  child.hidden = !open.has(entry.path);
  item.appendChild(child);
  line.addEventListener('click', async () => {
    if (open.has(entry.path)) {
      open.delete(entry.path);
      child.hidden = true;
      return;
    }
    open.add(entry.path);
    child.hidden = false;
    if (!child.childElementCount) await fill(child, entry.path);
  });
  return item;
}

async function fill(list, rel) {
  const listing = await invoke('list_workdir', { rel });
  list.replaceChildren();
  for (const entry of listing.entries) list.appendChild(row(entry));
}

async function refresh() {
  const settings = await invoke('get_settings');
  $('folder').textContent = settings.workdir ?? '';
  const root = document.createElement('ul');
  $('tree').replaceChildren(root);
  await fill(root, '');
}

$('web-only').addEventListener('click', () => invoke('set_native_shell', { enabled: false }));

const term = $('term');
listen('term-data', (event) => {
  term.textContent += event.payload;
  term.scrollTop = term.scrollHeight;
});
term.addEventListener('keydown', (event) => {
  let data = event.key;
  if (event.key === 'Enter') data = '\r';
  else if (event.key === 'Backspace') data = '\x7f';
  else if (event.key === 'Tab') data = '\t';
  else if (event.key.length !== 1 || event.metaKey || event.ctrlKey || event.altKey) return;
  event.preventDefault();
  invoke('term_write', { data });
});

listen('settings-changed', () => { refresh().catch(() => {}); });
refresh().catch((error) => { $('folder').textContent = String(error); });
