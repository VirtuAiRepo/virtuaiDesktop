// Settings: the server, the working folder and the connector. A page of the
// app's own, with IPC; the server's web chat never gets these commands.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const form = $('server-form');
const server = $('server');
const note = $('server-note');
let paused = false;

async function refresh() {
  const s = await invoke('get_settings');
  // Not while the person is typing a new address.
  if (document.activeElement !== server) server.value = s.server ?? '';
  $('workdir').textContent = s.workdir ?? '—';
  const text = s.connector_text.replace(/^Connector: /, '');
  $('connector').textContent = text.charAt(0).toUpperCase() + text.slice(1);
  $('connector').dataset.state = s.connector;
  paused = s.paused;
  $('pause').textContent = paused ? 'Resume' : 'Pause';
  $('version').textContent = s.version;
}

form.addEventListener('submit', async (event) => {
  event.preventDefault();
  note.className = 'note';
  note.textContent = '';
  const button = form.querySelector('button');
  button.disabled = true;
  try {
    const saved = await invoke('set_server', { url: server.value.trim() });
    server.value = saved;
    note.textContent = 'Saved. The chat now opens this server; sign in there.';
  } catch (e) {
    note.className = 'note error';
    note.textContent = String(e);
  } finally {
    button.disabled = false;
  }
});

$('open-folder').addEventListener('click', () => invoke('open_folder'));
$('choose-folder').addEventListener('click', () => invoke('choose_folder'));
$('pause').addEventListener('click', () => invoke('set_paused', { paused: !paused }));

listen('settings-changed', refresh);
refresh();
