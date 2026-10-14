// A command the runner's guard asks about, when a yes can stand: once, this
// conversation, an hour, or always in this folder (the CLI's guard_grants.py).
// A page of the app's own, with IPC; closing the window is a no.
const { invoke } = window.__TAURI__.core;

const $ = (id) => document.getElementById(id);
const id = new URLSearchParams(window.location.search).get('id') ?? '';
const buttons = Array.from(document.querySelectorAll('.choices button'));

async function load() {
  try {
    const view = await invoke('approval_details', { id });
    $('command').textContent = view.command;
    $('why').textContent = view.reason ? `Why you are asked: ${view.reason}` : '';
    $('folder').textContent = view.workdir;
    for (const el of document.querySelectorAll('.base')) el.textContent = view.base;
    // No base: asked every time, so nothing to grant beyond this once.
    if (!view.base) $('covers').hidden = true;
    for (const button of buttons) {
      const scope = button.dataset.scope;
      if (scope && scope !== 'once' && !view.scopes.includes(scope)) button.hidden = true;
    }
    // The safe answer has the focus: Enter is a no.
    document.querySelector('[data-deny]').focus();
  } catch (e) {
    $('note').className = 'note error';
    $('note').textContent = String(e);
    for (const button of buttons) button.disabled = true;
  }
}

for (const button of buttons) {
  button.addEventListener('click', async () => {
    for (const b of buttons) b.disabled = true;
    const approved = !('deny' in button.dataset);
    try {
      await invoke('answer_approval', { id, approved, scope: button.dataset.scope ?? 'once' });
    } catch (e) {
      $('note').className = 'note error';
      $('note').textContent = String(e);
    }
  });
}

document.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') document.querySelector('[data-deny]').click();
});

load();
