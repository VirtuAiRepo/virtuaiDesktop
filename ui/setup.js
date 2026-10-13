// The only page with IPC: it asks for the server address once. After that the
// window shows the server's own web chat, which has no access to the app.
const { invoke } = window.__TAURI__.core;

const form = document.getElementById('setup');
const input = document.getElementById('server');
const error = document.getElementById('error');

invoke('current_server').then((url) => { if (url) input.value = url; });

form.addEventListener('submit', async (event) => {
  event.preventDefault();
  error.textContent = '';
  const button = form.querySelector('button');
  button.disabled = true;
  try {
    await invoke('set_server', { url: input.value.trim() });
  } catch (e) {
    error.textContent = String(e);
    button.disabled = false;
  }
});
