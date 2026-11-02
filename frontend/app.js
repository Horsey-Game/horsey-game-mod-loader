const { invoke } = window.__TAURI__.core;
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);

let mods = [];
let horseyRunning = false;
let selectedDir = null;

function setStatus(text) {
  $('status-text').textContent = text;
}

function selectedMod() {
  return mods.find((m) => m.dir === selectedDir) ?? null;
}

function updateToggle() {
  const mod = selectedMod();
  const canToggle = mod && (mod.state === 'Enabled' || mod.state === 'Disabled');
  $('btn-toggle').disabled = !canToggle;
  $('btn-remove').disabled = !(mod && mod.dir);
}

function cell(text) {
  const td = document.createElement('td');
  td.textContent = text;
  return td;
}

function select(dir) {
  selectedDir = dir;
  document.querySelectorAll('#mod-rows tr').forEach((tr) => {
    tr.classList.toggle('highlighted', tr.dataset.dir === dir);
  });
  updateToggle();
}

function renderMods() {
  const body = $('mod-rows');
  body.replaceChildren(
    ...mods.map((mod) => {
      const tr = document.createElement('tr');
      tr.dataset.dir = mod.dir;
      tr.append(cell(mod.name), cell(mod.version), cell(mod.state));
      tr.addEventListener('click', () => select(mod.dir));
      return tr;
    })
  );
  $('status-count').textContent = `${mods.length} mods`;
  updateToggle();
}

async function refresh() {
  try {
    const status = await invoke('load_status');
    $('game-path').textContent =
      status.game_dir ?? 'Horsey Game was not found in your Steam libraries.';
    mods = status.mods;
    horseyRunning = status.horsey_running;
    if (!mods.some((m) => m.dir === selectedDir)) selectedDir = null;
    renderMods();
    setStatus('Ready');
  } catch (err) {
    setStatus(String(err));
  }
}

async function toggle() {
  const mod = selectedMod();
  if (!mod) return;
  const next = mod.state === 'Enabled' ? 'Disabled' : 'Enabled';
  try {
    await invoke('toggle_mod', { dir: mod.dir });
    await refresh();
    setStatus(`${mod.name}: ${next}`);
  } catch (err) {
    setStatus(String(err));
  }
}

$('btn-min').addEventListener('click', () => appWindow.minimize());
$('btn-max').addEventListener('click', () => appWindow.toggleMaximize());
$('btn-close').addEventListener('click', () => appWindow.close());
$('btn-toggle').addEventListener('click', toggle);
$('btn-refresh').addEventListener('click', refresh);

async function uninstall() {
  const mod = selectedMod();
  if (!mod || !mod.dir) return;
  if (!(await askConfirm(`Uninstall ${mod.name}? A backup of the game is made first.`))) return;
  try {
    await invoke('uninstall_mod', { dir: mod.dir });
    await refresh();
    setStatus(`Uninstalled ${mod.name}`);
  } catch (err) {
    setStatus(String(err));
  }
}

async function backToVanilla() {
  const question =
    'Restore mods and version.dll from the vanilla backup? Saves are not touched. ' +
    'A backup of the current state is made first.';
  if (!(await askConfirm(question))) return;
  try {
    await invoke('restore_vanilla');
    await refresh();
    setStatus('Restored vanilla mods and version.dll');
  } catch (err) {
    setStatus(String(err));
  }
}

$('btn-remove').addEventListener('click', uninstall);
$('btn-vanilla').addEventListener('click', backToVanilla);

function askConfirm(message) {
  return new Promise((resolve) => {
    const ok = $('confirm-ok');
    const cancel = $('confirm-cancel');
    const finish = (answer) => {
      $('confirm-modal').hidden = true;
      ok.removeEventListener('click', onOk);
      cancel.removeEventListener('click', onCancel);
      resolve(answer);
    };
    const onOk = () => finish(true);
    const onCancel = () => finish(false);
    $('confirm-text').textContent = message;
    $('confirm-modal').hidden = false;
    ok.addEventListener('click', onOk);
    cancel.addEventListener('click', onCancel);
    ok.focus();
  });
}

async function installZip(path) {
  const file = path.split(/[\\/]/).pop();
  if (!(await askConfirm(`${file} can run code inside Horsey. Only install mods you trust. Install it?`))) {
    return `${file}: install cancelled`;
  }
  try {
    const result = await invoke('install_mod', { zipPath: path, overwrite: false });
    const shim = result.shim_installed ? ' (shim installed)' : '';
    return `Installed ${result.name}${shim}`;
  } catch (err) {
    if (String(err) !== 'already installed') return `${file}: ${err}`;
    if (!(await askConfirm(`${file} is already installed. Replace it?`))) {
      return `${file}: install cancelled`;
    }
    try {
      const result = await invoke('install_mod', { zipPath: path, overwrite: true });
      return `Replaced ${result.name}`;
    } catch (retryErr) {
      return `${file}: ${retryErr}`;
    }
  }
}

appWindow.onDragDropEvent(async (event) => {
  if (event.payload.type !== 'drop') return;
  const zips = event.payload.paths.filter((p) => p.toLowerCase().endsWith('.zip'));
  if (zips.length === 0) {
    setStatus('Drop a mod .zip onto the window to install it');
    return;
  }
  const messages = [];
  for (const zip of zips) messages.push(await installZip(zip));
  await refresh();
  setStatus(messages.join(' | '));
});


let remoteMods = [];
let selectedRemoteId = null;

function showView(name) {
  document.querySelectorAll('.view').forEach((view) => {
    view.hidden = view.id !== `view-${name}`;
  });
  document.querySelectorAll('.tabs li').forEach((li) => {
    li.setAttribute('aria-selected', String(li.id === `tab-${name}`));
  });
  if (name === 'browse' && remoteMods.length === 0) loadRemote();
}

function remoteStatus(mod) {
  const local = mods.find((m) => m.folder === mod.id);
  if (!local) return 'Available';
  return local.version === mod.version ? 'Installed' : 'Update available';
}

function actionLabel(status) {
  if (status === 'Available') return 'Install';
  if (status === 'Update available') return 'Update';
  return 'Reinstall';
}

function selectedRemote() {
  return remoteMods.find((m) => m.id === selectedRemoteId) ?? null;
}

function selectRemote(id) {
  selectedRemoteId = id;
  document.querySelectorAll('#remote-rows tr').forEach((tr) => {
    tr.classList.toggle('highlighted', tr.dataset.id === id);
  });
  const mod = selectedRemote();
  $('remote-details').textContent = mod
    ? [
        `${mod.name} ${mod.version} by ${mod.author}`,
        '',
        mod.description,
        '',
        `Needs API ${mod.api_version}  |  ${Math.max(1, Math.round(mod.size / 1024))} KB`,
        `Dependencies: ${
          mod.dependencies.length
            ? mod.dependencies.map((d) => (typeof d === 'string' ? d : JSON.stringify(d))).join(', ')
            : 'none'
        }`,
        `SHA-256: ${mod.sha256}`,
      ].join('\n')
    : 'Select a mod to see its details.';
  $('btn-install').disabled = !mod;
  $('btn-install').textContent = mod ? actionLabel(remoteStatus(mod)) : 'Install';
}

function renderRemote() {
  const body = $('remote-rows');
  body.replaceChildren(
    ...remoteMods.map((mod) => {
      const tr = document.createElement('tr');
      tr.dataset.id = mod.id;
      tr.append(cell(mod.name), cell(mod.version), cell(mod.author), cell(remoteStatus(mod)));
      tr.addEventListener('click', () => selectRemote(mod.id));
      return tr;
    })
  );
  selectRemote(selectedRemoteId);
}

async function loadRemote() {
  setStatus('Loading mod list...');
  try {
    remoteMods = await invoke('fetch_index');
    renderRemote();
    const updates = remoteMods.filter((m) => remoteStatus(m) === 'Update available').length;
    setStatus(updates ? `${updates} update(s) available` : `${remoteMods.length} mod(s) available`);
  } catch (err) {
    setStatus(`Could not load mod list: ${err}`);
  }
}

async function installRemoteMod(mod) {
  const isUpdate = remoteStatus(mod) === 'Update available';
  const args = { url: mod.download_url, overwrite: isUpdate };
  try {
    const result = await invoke('install_remote', args);
    const shim = result.shim_installed ? ' (shim installed)' : '';
    return `Installed ${result.name}${shim}`;
  } catch (err) {
    if (String(err) !== 'already installed') return `${mod.name}: ${err}`;
    if (!(await askConfirm(`${mod.name} is already installed. Replace it?`))) {
      return `${mod.name}: install cancelled`;
    }
    try {
      const result = await invoke('install_remote', { ...args, overwrite: true });
      return `Replaced ${result.name}`;
    } catch (retryErr) {
      return `${mod.name}: ${retryErr}`;
    }
  }
}

async function installSelectedRemote() {
  const mod = selectedRemote();
  if (!mod) return;
  const message = await installRemoteMod(mod);
  await refresh();
  renderRemote();
  setStatus(message);
}

document.querySelectorAll('.tabs a').forEach((a) => {
  a.addEventListener('click', (e) => {
    e.preventDefault();
    showView(a.dataset.view);
  });
});
$('btn-install').addEventListener('click', installSelectedRemote);
$('btn-browse-refresh').addEventListener('click', loadRemote);

async function offerShim() {
  const shim = mods.find((m) => m.folder === 'version.dll');
  if (!shim || horseyRunning) return;
  // update-popup nur wenn die liste eine neuere shim hat
  if (shim.state === 'Installed' || shim.state === 'Shim update') {
    let available = false;
    try {
      available = await invoke('shim_update_available');
    } catch (err) {
      setStatus(String(err));
      return;
    }
    if (!available) return;
    if (!(await askConfirm('A new Horsey shim is available. Update to it? A backup of the game folder is made first.'))) {
      setStatus('Shim update skipped.');
      return;
    }
    try {
      await invoke('update_shim', { force: true });
      await refresh();
      setStatus('Shim updated');
    } catch (err) {
      setStatus(String(err));
    }
    return;
  }
  const question =
    shim.state === 'Other version.dll'
      ? 'Mods need the Horsey shim, and a different version.dll is in the game folder. ' +
        'Replace it with the shim? A backup of the game folder is made first.'
      : 'Mods need the Horsey shim, and it is not installed yet. Install it now?';
  if (!(await askConfirm(question))) {
    setStatus('Shim not installed. Mods will not load until it is.');
    return;
  }
  try {
    await invoke('install_shim');
    await refresh();
    setStatus('Shim installed');
  } catch (err) {
    setStatus(String(err));
  }
}

// manager-update anbieten, aber nie erzwingen
async function offerManagerUpdate() {
  let update = null;
  try {
    update = await invoke('manager_update_available');
  } catch (err) {
    return;
  }
  if (!update) return;
  const question =
    `Version ${update.version} of the mod manager is available. You have ${update.current}. ` +
    'Update now? The manager closes and the installer opens.';
  if (!(await askConfirm(question))) {
    setStatus('Manager update skipped.');
    return;
  }
  try {
    await invoke('apply_manager_update');
  } catch (err) {
    setStatus(String(err));
  }
}

invoke('update_shim', { force: false })
  .then(() => '', (err) => String(err))
  .then(async (shimError) => {
    await refresh();
    await offerShim();
    await offerManagerUpdate();
    await loadRemote();
    if (shimError) setStatus(`Shim update failed: ${shimError}`);
  });
