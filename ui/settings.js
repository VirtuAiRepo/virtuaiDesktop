// Settings: the server, the working folder and the connector. A page of the
// app's own, with IPC; the server's web chat never gets these commands.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const form = $('server-form');
const server = $('server');
const note = $('server-note');
const agentForm = $('agent-form');
const agent = $('agent');
const agentNote = $('agent-note');
let paused = false;

async function refresh() {
  const s = await invoke('get_settings');
  // Not while the person is typing a new address.
  if (document.activeElement !== server) server.value = s.server ?? '';
  if (document.activeElement !== agent) agent.value = s.agent ?? '';
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
    note.textContent = 'Saved. Sign in there. The IDE opens here once the connector is connected and an agent is set.';
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

agentForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  agentNote.className = 'note';
  agentNote.textContent = '';
  const button = agentForm.querySelector('button');
  button.disabled = true;
  try {
    await invoke('set_agent', { agent: agent.value.trim() });
    agentNote.textContent = 'Saved. The IDE opens once the connector is connected.';
  } catch (e) {
    agentNote.className = 'note error';
    agentNote.textContent = String(e);
  } finally {
    button.disabled = false;
  }
});

$('open-console').addEventListener('click', async () => {
  agentNote.className = 'note';
  try {
    await invoke('open_console');
  } catch (e) {
    agentNote.className = 'note error';
    agentNote.textContent = String(e);
  }
});

listen('settings-changed', refresh);
refresh();
