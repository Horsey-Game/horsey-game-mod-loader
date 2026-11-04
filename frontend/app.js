const { invoke: rawInvoke } = window.__TAURI__.core;
const invoke = (cmd, args) =>
  rawInvoke(cmd, args).catch((err) => {
    rawInvoke('app_log', { line: `error ${cmd}: ${err}` }).catch(() => {});
    throw err;
  });
window.addEventListener('error', (e) =>
  rawInvoke('app_log', { line: `js error: ${e.message}` }).catch(() => {}),
);
window.addEventListener('unhandledrejection', (e) =>
  rawInvoke('app_log', { line: `unhandled: ${e.reason}` }).catch(() => {}),
);
const appWindow = window.__TAURI__.window.getCurrentWindow();

const $ = (id) => document.getElementById(id);

let mods = [];
let extras = { shim_api: 2, mods: [] };
let horseyRunning = false;
let selectedDir = null;
// standard, bis die gespeicherten einstellungen geladen sind
let settings = { check_updates: true, backups_keep: 5, game_dir: '' };
let remoteMods = [];
let selectedRemoteId = null;
// laufender bisect-schritt, null wenn keiner läuft
let bisect = null;
// mod, dessen config gerade im fenster offen ist
let configDir = null;
const IDLE_BISECT = { active: false, testing: [], suspects_left: 0, culprit: null };

function setStatus(text) {
  rawInvoke('app_log', { line: `status: ${text}` }).catch(() => {});
  $('status-text').textContent = text;
}

function cell(text) {
  const td = document.createElement('td');
  td.textContent = text;
  return td;
}

function selectedMod() {
  return mods.find((m) => m.dir === selectedDir) ?? null;
}

function extraFor(mod) {
  return extras.mods.find((e) => e.folder === mod.folder) ?? null;
}

function shimRow() {
  return mods.find((m) => m.folder === 'version.dll') ?? null;
}

function remoteFor(id) {
  return remoteMods.find((r) => r.id === id) ?? null;
}

function modNameFor(folder) {
  const row = mods.find((m) => m.folder === folder);
  return row ? row.name : folder;
}

// abhängigkeit als text, egal ob nur id oder {"id": ...}
function depName(dep) {
  return typeof dep === 'string' ? dep : dep.id ?? JSON.stringify(dep);
}

// zahlenweise vergleich, 0.1.10 ist neuer als 0.1.9
function compareVersions(a, b) {
  const pa = String(a).split('.').map((p) => parseInt(p, 10) || 0);
  const pb = String(b).split('.').map((p) => parseInt(p, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] || 0) - (pb[i] || 0);
    if (d) return d;
  }
  return 0;
}

// 'ok' = an, 'off' = installiert aber aus, 'missing' = nicht installiert
function depState(depId) {
  const extra = extras.mods.find((e) => e.id === depId);
  if (!extra) return 'missing';
  const row = mods.find((m) => m.folder === extra.folder);
  if (!row) return 'missing';
  if (row.state === 'Enabled') return 'ok';
  return row.state === 'Disabled' ? 'off' : 'missing';
}

// abhängigkeiten, die nicht an sind
function depProblems(extra) {
  if (!extra) return [];
  return extra.deps.filter((d) => depState(d) !== 'ok');
}

// hinweise für die liste: api, abhängigkeiten, updates
function notesFor(mod) {
  const extra = extraFor(mod);
  if (!extra) return [];
  const notes = [];
  if (extra.api_version > extras.shim_api) {
    notes.push('needs newer manager');
  } else if (extra.api_version >= extras.shim_api && shimRow()?.state !== 'Installed') {
    notes.push('needs shim update');
  }
  for (const dep of extra.deps) {
    const state = depState(dep);
    if (state === 'missing') notes.push(`missing: ${dep}`);
    if (state === 'off') notes.push(`${dep} is off`);
  }
  const remote = remoteFor(extra.id);
  if (remote && compareVersions(remote.version, mod.version) > 0) {
    notes.push(`update ${remote.version}`);
  }
  return notes;
}

function updateDetails() {
  const mod = selectedMod();
  const box = $('installed-details');
  if (!mod || !mod.dir) {
    box.textContent = 'Select a mod to see its details.';
    return;
  }
  const extra = extraFor(mod);
  const notes = notesFor(mod);
  box.textContent = [
    `${mod.name} ${mod.version}`,
    extra ? `Id: ${extra.id}   API: ${extra.api_version}   Config: ${extra.has_config ? 'yes' : 'no'}` : '',
    `Dependencies: ${extra && extra.deps.length ? extra.deps.join(', ') : 'none'}`,
    `Notes: ${notes.length ? notes.join('; ') : 'none'}`,
  ]
    .filter(Boolean)
    .join('\n');
}

function updateToggle() {
  const mod = selectedMod();
  const canToggle = mod && (mod.state === 'Enabled' || mod.state === 'Disabled');
  $('btn-toggle').disabled = !canToggle;
  $('btn-remove').disabled = !(mod && mod.dir);
  const extra = mod ? extraFor(mod) : null;
  $('btn-config').disabled = !(extra && extra.has_config);
  updateDetails();
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
      tr.append(cell(mod.name), cell(mod.version), cell(mod.state), cell(notesFor(mod).join('; ')));
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
      status.game_dir ?? 'Horsey Game was not found. Set the game folder in Settings.';
    mods = status.mods;
    horseyRunning = status.horsey_running;
    extras = await invoke('mod_extras');
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
  // beim einschalten warnen, wenn abhängigkeiten fehlen oder aus sind
  if (next === 'Enabled') {
    const problems = depProblems(extraFor(mod));
    const question = `${mod.name} needs ${problems.join(', ')}, which is missing or off. Enable it anyway?`;
    if (problems.length && !(await askConfirm(question))) return;
  }
  try {
    await invoke('toggle_mod', { dir: mod.dir });
    await refresh();
    setStatus(`${mod.name}: ${next}`);
  } catch (err) {
    setStatus(String(err));
  }
}

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

// ja/nein-fenster, die beschriftung ist austauschbar (z.b. für bisect)
function askConfirm(message, okLabel = 'OK', cancelLabel = 'Cancel') {
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
    ok.textContent = okLabel;
    cancel.textContent = cancelLabel;
    $('confirm-modal').hidden = false;
    ok.addEventListener('click', onOk);
    cancel.addEventListener('click', onCancel);
    ok.focus();
  });
}

// config-fenster: ein feld pro zeile in config.lua
function configRow(field, entry) {
  const row = document.createElement('div');
  row.className = 'field-row config-row';
  const input = document.createElement('input');
  input.id = `cfg-${field.key}`;
  input.dataset.key = field.key;
  input.dataset.kind = field.kind;
  if (field.kind === 'bool') {
    input.type = 'checkbox';
    input.checked = field.value === 'true';
  } else if (field.kind === 'number') {
    input.type = 'number';
    input.step = 'any';
    if (entry?.min !== undefined) input.min = entry.min;
    if (entry?.max !== undefined) input.max = entry.max;
    input.value = field.value;
  } else {
    input.type = 'text';
    input.value = field.value;
  }
  const label = document.createElement('label');
  label.textContent = entry?.label ?? field.key;
  row.title = entry?.description ?? '';
  label.htmlFor = input.id;
  row.append(label, input);
  return row;
}

async function openConfig() {
  const mod = selectedMod();
  if (!mod) return;
  try {
    const fields = await invoke('read_config', { dir: mod.dir });
    configDir = mod.dir;
    $('config-title').textContent = `${mod.name} config`;
    $('config-note').textContent =
      'Changes apply the next time Horsey starts.' + (horseyRunning ? ' Horsey is running now.' : '');
    // beschriftung und grenzen aus dem manifest, falls der mod welche angibt
    const raw = extraFor(mod)?.config_schema;
    const schema = Array.isArray(raw) ? raw : [];
    $('config-fields').replaceChildren(
      ...fields.map((f) => configRow(f, schema.find((s) => s.key === f.key)))
    );
    $('config-modal').hidden = false;
  } catch (err) {
    setStatus(String(err));
  }
}

async function saveConfig() {
  const fields = [...$('config-fields').querySelectorAll('input')].map((input) => ({
    key: input.dataset.key,
    kind: input.dataset.kind,
    value: input.dataset.kind === 'bool' ? String(input.checked) : input.value,
  }));
  try {
    await invoke('write_config', { dir: configDir, fields });
    $('config-modal').hidden = true;
    setStatus('Config saved. Restart Horsey for it to apply.');
  } catch (err) {
    // fenster bleibt offen, damit der fehler korrigiert werden kann
    setStatus(String(err));
  }
}

async function launchHorsey() {
  try {
    await invoke('launch_horsey');
    horseyRunning = true;
    setStatus('Horsey started. Its exit is checked for crashes.');
  } catch (err) {
    setStatus(String(err));
  }
}

// exit-code als text, negative werte als hex (windows-fehlercodes)
function codeText(code) {
  if (code === null || code === undefined) return 'none (it was stopped)';
  return code < 0 ? `${code} (0x${(code >>> 0).toString(16).toUpperCase()})` : String(code);
}

function crashText(report) {
  // ohne exit-code (gestartet außerhalb des managers) kann man nichts über den grund sagen
  const lines = report.exit_known
    ? [
        `Horsey closed with code ${codeText(report.code)}.`,
        report.crashed ? 'That looks like a crash.' : 'It closed normally.',
      ]
    : ['Horsey was started outside the manager, so its exit code is unknown.'];
  if (report.culprit) {
    lines.push(`Last thing a mod logged before it closed came from ${report.culprit}:`, report.line);
  } else {
    lines.push('No mod logged anything before it closed, so a mod may not be the cause.');
  }
  lines.push('This is a hint, not proof. Crash bisect can confirm it.');
  return lines.join('\n');
}

// bisect-anzeige: läuft er, ist er fertig oder gar nicht gestartet
function showBisect(step) {
  bisect = step.active ? step : null;
  if (step.active) {
    $('bisect-status').textContent =
      `Testing: ${step.testing.map(modNameFor).join(', ')}\nSuspects left: ${step.suspects_left}`;
  } else if (step.culprit) {
    $('bisect-status').textContent = `Found it: ${modNameFor(step.culprit)}. All mods are back on.`;
  } else {
    $('bisect-status').textContent = 'Not running.';
  }
  $('btn-bisect-start').disabled = step.active;
  $('btn-bisect-crash').disabled = !step.active;
  $('btn-bisect-ok').disabled = !step.active;
  $('btn-bisect-cancel').disabled = !step.active;
}

async function refreshBisect() {
  try {
    showBisect((await invoke('bisect_status')) ?? IDLE_BISECT);
  } catch (err) {
    setStatus(String(err));
  }
}

async function bisectStart() {
  try {
    showBisect(await invoke('bisect_start'));
    setStatus('Bisect started. Launch Horsey and say whether it crashed.');
  } catch (err) {
    setStatus(String(err));
  }
  await refresh();
}

async function answerBisect(crashed) {
  try {
    const step = await invoke('bisect_answer', { crashed });
    showBisect(step);
    if (step.culprit) {
      setStatus(`Found it: ${modNameFor(step.culprit)}. Mods were restored.`);
    } else {
      setStatus('Next test is ready. Launch Horsey.');
    }
  } catch (err) {
    setStatus(String(err));
  }
  await refresh();
}

async function bisectCancel() {
  try {
    await invoke('bisect_cancel');
    showBisect(IDLE_BISECT);
    setStatus('Bisect cancelled. Mods were restored.');
  } catch (err) {
    setStatus(String(err));
  }
  await refresh();
}

// horsey wurde über einen launch-knopf gestartet und ist jetzt zu
window.__TAURI__.event.listen('horsey-exited', async (event) => {
  const report = event.payload;
  horseyRunning = false;
  refreshCrashHistory();
  $('crash-report').textContent = crashText(report);
  if (bisect) {
    const crashed = await askConfirm(
      `${crashText(report)}\n\nDid it crash during this bisect test?`,
      'It Crashed',
      'No Crash',
    );
    await answerBisect(crashed);
    return;
  }
  setStatus(report.crashed ? `Horsey crashed (code ${codeText(report.code)}).` : 'Horsey closed.');
  await refresh();
});

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

function showView(name) {
  document.querySelectorAll('.view').forEach((view) => {
    view.hidden = view.id !== `view-${name}`;
  });
  document.querySelectorAll('.tabs li').forEach((li) => {
    li.setAttribute('aria-selected', String(li.id === `tab-${name}`));
  });
  if (name === 'browse' && remoteMods.length === 0) loadRemote();
  if (name === 'settings') loadSettingsForm();
  if (name === 'crashes') {
    refreshBisect();
    refreshCrashHistory();
    openLog();
  } else {
    stopLogTimer();
  }
  if (name === 'tools') refreshTools();
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

// abhängigkeiten eines online-mods mit ihrem status
function remoteDepsText(mod) {
  if (!mod.dependencies.length) return 'none';
  return mod.dependencies
    .map((d) => {
      const id = depName(d);
      const state = depState(id);
      if (state === 'ok') return id;
      return state === 'off' ? `${id} (off)` : `${id} (not installed)`;
    })
    .join(', ');
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
        `Dependencies: ${remoteDepsText(mod)}`,
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
    // installierte liste bekommt update-hinweise aus der neuen liste
    renderMods();
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
  // fehlende abhängigkeiten vorher ansagen
  const missing = mod.dependencies.map(depName).filter((d) => depState(d) === 'missing');
  if (
    missing.length &&
    !(await askConfirm(`${mod.name} needs ${missing.join(', ')}, which is not installed. Install it anyway?`))
  ) {
    setStatus(`${mod.name}: install cancelled`);
    return;
  }
  const message = await installRemoteMod(mod);
  await refresh();
  renderRemote();
  setStatus(message);
}

// einstellungen ins formular laden
async function loadSettingsForm() {
  try {
    settings = await invoke('get_settings');
    $('set-game-dir').value = settings.game_dir;
    $('set-check-updates').checked = settings.check_updates;
    $('set-backups').value = settings.backups_keep;
  } catch (err) {
    setStatus(String(err));
  }
}

async function saveSettings() {
  const next = {
    game_dir: $('set-game-dir').value.trim(),
    check_updates: $('set-check-updates').checked,
    backups_keep: Number($('set-backups').value) || 5,
  };
  try {
    await invoke('save_settings', { settings: next });
    settings = next;
    await refresh();
    setStatus('Settings saved');
  } catch (err) {
    setStatus(String(err));
  }
}

async function exportDiagnostics() {
  setStatus('Writing diagnostics...');
  try {
    const path = await invoke('export_diagnostics');
    setStatus(`Diagnostics saved to ${path}`);
  } catch (err) {
    setStatus(String(err));
  }
}

async function offerShim() {
  const shim = shimRow();
  if (!shim || horseyRunning) return;
  // update-popup nur wenn die liste eine neuere shim hat und das erlaubt ist
  if (shim.state === 'Installed' || shim.state === 'Shim update') {
    if (!settings.check_updates) return;
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
  if (!settings.check_updates) return;
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
    await invoke('apply_manager_update').catch((err) => {
      alert(`Manager update failed:\n${err}\n\nLog: %LOCALAPPDATA%\\com.chris.horseymodmanager\\update.log`);
    });
  } catch (err) {
    setStatus(String(err));
  }
}

// knöpfe und tabs verdrahten
$('btn-min').addEventListener('click', () => appWindow.minimize());
$('btn-max').addEventListener('click', () => appWindow.toggleMaximize());
$('btn-close').addEventListener('click', () => appWindow.close());
$('btn-toggle').addEventListener('click', toggle);
$('btn-refresh').addEventListener('click', refresh);
$('btn-remove').addEventListener('click', uninstall);
$('btn-config').addEventListener('click', openConfig);
$('btn-vanilla').addEventListener('click', backToVanilla);
$('config-save').addEventListener('click', saveConfig);
$('config-cancel').addEventListener('click', () => {
  $('config-modal').hidden = true;
});
document.querySelectorAll('.btn-launch').forEach((btn) => btn.addEventListener('click', launchHorsey));
$('btn-bisect-start').addEventListener('click', bisectStart);
$('btn-bisect-crash').addEventListener('click', () => answerBisect(true));
$('btn-bisect-ok').addEventListener('click', () => answerBisect(false));
$('btn-bisect-cancel').addEventListener('click', bisectCancel);
document.querySelectorAll('.tabs a').forEach((a) => {
  a.addEventListener('click', (e) => {
    e.preventDefault();
    showView(a.dataset.view);
  });
});
$('btn-install').addEventListener('click', installSelectedRemote);
$('btn-browse-refresh').addEventListener('click', loadRemote);
$('btn-settings-save').addEventListener('click', saveSettings);
$('btn-diagnostics').addEventListener('click', exportDiagnostics);
$('btn-open-logs').addEventListener('click', () =>
  invoke('open_log_dir').catch((err) => setStatus(String(err))),
);

// start: erst shim prüfen, dann einstellungen, dann updates anbieten
invoke('update_shim', { force: false })
  .then(() => '', (err) => String(err))
  .then(async (shimError) => {
    await refresh();
    settings = await invoke('get_settings');
    await refreshBisect();
    await checkGameChanged();
    await offerShim();
    await offerManagerUpdate();
    await loadRemote();
    if (shimError) setStatus(`Shim update failed: ${shimError}`);
  });

// --- update all: alle mods mit update-hinweis nacheinander installieren ---
async function updateAll() {
  const updates = remoteMods.filter((m) => remoteStatus(m) === 'Update available');
  if (!updates.length) {
    setStatus('Nothing to update');
    return;
  }
  const names = updates.map((m) => m.name).join(', ');
  if (!(await askConfirm(`Update ${updates.length} mod(s): ${names}?`))) return;
  const messages = [];
  for (const mod of updates) messages.push(await installRemoteMod(mod));
  await refresh();
  renderRemote();
  setStatus(messages.join(' | '));
}

// --- werkzeuge-tab ---
let selectedBackup = null;
let selectedProfile = null;

function refreshTools() {
  checkIntegrity();
  refreshBackups();
  refreshProfiles();
  // ungespeicherte änderungen an der reihenfolge bleiben stehen
  if (!orderDirty) refreshOrder();
}

async function checkIntegrity() {
  try {
    const rows = await invoke('check_integrity');
    $('integrity-rows').replaceChildren(
      ...rows.map((r) => {
        const tr = document.createElement('tr');
        tr.append(cell(r.name), cell(r.status), cell(r.problems.slice(0, 3).join('; ')));
        return tr;
      })
    );
    const bad = rows.filter((r) => r.status === 'modified').length;
    const unrecorded = rows.filter((r) => r.status === 'no record').length;
    setStatus(
      bad
        ? `${bad} mod(s) changed since they were installed`
        : unrecorded
          ? `${unrecorded} mod(s) have no record yet. Record Baseline sets one.`
          : 'Mod files match what was installed'
    );
  } catch (err) {
    setStatus(String(err));
  }
}

async function recordBaseline() {
  const question =
    'Record the current files of mods without a record as correct? Only do this if they are not modified.';
  if (!(await askConfirm(question))) return;
  try {
    const count = await invoke('record_baseline');
    setStatus(`Recorded the current files for ${count} mod(s)`);
    await checkIntegrity();
  } catch (err) {
    setStatus(String(err));
  }
}

// backup-zeitstempel als datum, vanilla hat keinen
function fmtStamp(stamp) {
  return stamp ? new Date(stamp * 1000).toLocaleString() : 'Original';
}

function updateBackupButtons() {
  $('btn-backup-restore').disabled = !selectedBackup;
}

function selectBackup(name) {
  selectedBackup = name;
  document.querySelectorAll('#backup-rows tr').forEach((tr) => {
    tr.classList.toggle('highlighted', tr.dataset.name === name);
  });
  updateBackupButtons();
}

async function refreshBackups() {
  try {
    const rows = await invoke('list_backups');
    selectedBackup = null;
    $('backup-rows').replaceChildren(
      ...rows.map((b) => {
        const tr = document.createElement('tr');
        tr.dataset.name = b.name;
        tr.append(
          cell(fmtStamp(b.stamp)),
          cell(b.vanilla ? 'Original' : 'Backup'),
          cell(String(b.mods.length)),
          cell(String(b.differs))
        );
        tr.addEventListener('click', () => selectBackup(b.name));
        return tr;
      })
    );
    updateBackupButtons();
  } catch (err) {
    setStatus(String(err));
  }
}

async function restoreBackup() {
  if (!selectedBackup) return;
  const question =
    `Restore the mods and version.dll from ${fmtStamp(selectedBackup.startsWith('backup-') ? Number(selectedBackup.slice(7)) : 0)}? ` +
    'A backup of the current state is made first.';
  if (!(await askConfirm(question))) return;
  try {
    await invoke('restore_backup', { name: selectedBackup });
    await refresh();
    await refreshBackups();
    setStatus(`Restored ${selectedBackup}`);
  } catch (err) {
    setStatus(String(err));
  }
}

function updateProfileButtons() {
  $('btn-profile-apply').disabled = !selectedProfile;
  $('btn-profile-delete').disabled = !selectedProfile;
}

function selectProfile(name) {
  selectedProfile = name;
  document.querySelectorAll('#profile-rows tr').forEach((tr) => {
    tr.classList.toggle('highlighted', tr.dataset.name === name);
  });
  updateProfileButtons();
}

async function refreshProfiles() {
  try {
    const rows = await invoke('list_profiles');
    selectedProfile = null;
    $('profile-rows').replaceChildren(
      ...rows.map((p) => {
        const tr = document.createElement('tr');
        tr.dataset.name = p.name;
        tr.append(cell(p.name), cell(String(p.mods.length)));
        tr.addEventListener('click', () => selectProfile(p.name));
        return tr;
      })
    );
    updateProfileButtons();
  } catch (err) {
    setStatus(String(err));
  }
}

async function saveProfile() {
  const name = $('profile-name').value.trim();
  if (!name) {
    setStatus('Type a profile name first');
    return;
  }
  try {
    await invoke('save_profile', { name });
    $('profile-name').value = '';
    await refreshProfiles();
    setStatus(`Saved profile ${name}`);
  } catch (err) {
    setStatus(String(err));
  }
}

async function applyProfile() {
  if (!selectedProfile) return;
  if (!(await askConfirm(`Switch to ${selectedProfile}? Mods that are not in it are turned off.`))) return;
  try {
    await invoke('apply_profile', { name: selectedProfile });
    await refresh();
    setStatus(`Applied ${selectedProfile}`);
  } catch (err) {
    setStatus(String(err));
  }
}

async function deleteProfile() {
  if (!selectedProfile) return;
  if (!(await askConfirm(`Delete the profile ${selectedProfile}? Your mods are not changed.`))) return;
  try {
    await invoke('delete_profile', { name: selectedProfile });
    await refreshProfiles();
    setStatus('Profile deleted');
  } catch (err) {
    setStatus(String(err));
  }
}

async function checkGame() {
  try {
    const v = await invoke('game_version_status');
    const lines = [
      v.known
        ? v.changed
          ? 'Horsey.exe changed since it was accepted.'
          : 'Horsey.exe matches the accepted version.'
        : 'No accepted version yet. Accept it once Horsey works.',
      `Size: ${v.size} bytes`,
    ];
    if (v.sha256) lines.push(`SHA-256: ${v.sha256}`);
    $('game-status').textContent = lines.join('\n');
  } catch (err) {
    setStatus(String(err));
  }
}

async function acceptGame() {
  if (!(await askConfirm('Accept the current Horsey.exe as the known version?'))) return;
  try {
    await invoke('accept_game_version');
    await checkGame();
    setStatus('Game version accepted');
  } catch (err) {
    setStatus(String(err));
  }
}

// signaturen wie in check_sig.py: jede muss genau einmal vorkommen
async function checkSignatures() {
  try {
    const rows = await invoke('shim_signatures');
    const bad = rows.filter((r) => !r.ok).length;
    const lines = rows.map((r) => `${r.ok ? 'OK  ' : 'FAIL'} ${r.name}: ${r.matches} match(es)`);
    lines.push(
      bad
        ? `${bad} signature(s) do not match exactly once. The shim may break in this game version.`
        : 'All shim signatures match exactly once.'
    );
    $('game-status').textContent = lines.join('\n');
    setStatus(bad ? 'Shim signatures do not match' : 'Shim signatures match');
  } catch (err) {
    setStatus(String(err));
  }
}

// beim start: wenn horsey.exe sich geändert hat, sofort auf die signaturen schauen
async function checkGameChanged() {
  try {
    const v = await invoke('game_version_status');
    if (!v.changed) return;
    setStatus('Horsey.exe changed since it was accepted. Checking the shim.');
    showView('tools');
    await checkSignatures();
  } catch (err) {
    setStatus(String(err));
  }
}

async function safeModeLaunch() {
  const question = 'Start Horsey with every mod off? Your mods are turned back on when it closes.';
  if (!(await askConfirm(question))) return;
  try {
    await invoke('safe_mode_launch');
    horseyRunning = true;
    setStatus('Horsey started in safe mode');
  } catch (err) {
    setStatus(String(err));
  }
}

// knöpfe der neuen funktionen
$('btn-update-all').addEventListener('click', updateAll);
$('btn-integrity-check').addEventListener('click', checkIntegrity);
$('btn-integrity-baseline').addEventListener('click', recordBaseline);
$('btn-backup-restore').addEventListener('click', restoreBackup);
$('btn-backup-refresh').addEventListener('click', refreshBackups);
$('btn-profile-save').addEventListener('click', saveProfile);
$('btn-profile-apply').addEventListener('click', applyProfile);
$('btn-profile-delete').addEventListener('click', deleteProfile);
$('btn-game-check').addEventListener('click', checkGame);
$('btn-game-accept').addEventListener('click', acceptGame);
$('btn-sigs-check').addEventListener('click', checkSignatures);
$('btn-safe-launch').addEventListener('click', safeModeLaunch);

// --- log-ansicht: letzte zeilen von SHIM.log, optional nur von einem mod ---
let logTimer = null;

function populateLogFilter() {
  const select = $('log-filter');
  const current = select.value;
  const options = extras.mods.map((e) => new Option(modNameFor(e.folder), e.id));
  select.replaceChildren(new Option('All lines', ''), ...options);
  select.value = current;
}

async function refreshLog() {
  const box = $('log-view');
  try {
    const lines = await invoke('shim_log', { lines: 300, modId: $('log-filter').value || null });
    // nur nach unten springen, wenn man schon unten war
    const atBottom = box.scrollTop + box.clientHeight >= box.scrollHeight - 8;
    box.textContent = lines.length ? lines.join('\n') : 'No lines for this filter yet.';
    if (atBottom) box.scrollTop = box.scrollHeight;
  } catch (err) {
    box.textContent = String(err);
  }
}

function openLog() {
  populateLogFilter();
  refreshLog();
  if (logTimer === null) logTimer = setInterval(refreshLog, 5000);
}

function stopLogTimer() {
  if (logTimer !== null) clearInterval(logTimer);
  logTimer = null;
}

// --- absturzverlauf ---
function historyResult(h) {
  if (!h.exit_known) return 'closed (not from manager)';
  if (h.crashed) return `crashed (${codeText(h.code)})`;
  return 'closed';
}

async function refreshCrashHistory() {
  try {
    const rows = await invoke('crash_history');
    $('history-rows').replaceChildren(
      ...rows
        .slice()
        .reverse()
        .map((h) => {
          const tr = document.createElement('tr');
          tr.title = h.line ?? '';
          tr.append(
            cell(new Date(h.time * 1000).toLocaleString()),
            cell(historyResult(h)),
            cell(h.culprit ?? '-')
          );
          return tr;
        })
    );
  } catch (err) {
    setStatus(String(err));
  }
}

async function clearHistory() {
  if (!(await askConfirm('Clear the exit history? This does not change any mods.'))) return;
  try {
    await invoke('clear_crash_history');
    await refreshCrashHistory();
  } catch (err) {
    setStatus(String(err));
  }
}

$('log-filter').addEventListener('change', refreshLog);
$('btn-log-refresh').addEventListener('click', refreshLog);
$('btn-history-clear').addEventListener('click', clearHistory);

// --- load order: reihenfolge der mods, speichern in mods/load_order.txt ---
let orderFolders = [];
let selectedOrder = null;
let orderDirty = false;

async function refreshOrder() {
  try {
    orderFolders = await invoke('load_order');
    orderDirty = false;
    selectedOrder = null;
    renderOrder();
  } catch (err) {
    setStatus(String(err));
  }
}

function updateOrderButtons() {
  const i = orderFolders.indexOf(selectedOrder);
  $('btn-order-up').disabled = i <= 0;
  $('btn-order-down').disabled = i < 0 || i >= orderFolders.length - 1;
  $('btn-order-save').disabled = !orderDirty;
}

function selectOrder(folder) {
  selectedOrder = folder;
  document.querySelectorAll('#order-rows tr').forEach((tr) => {
    tr.classList.toggle('highlighted', tr.dataset.folder === folder);
  });
  updateOrderButtons();
}

function renderOrder() {
  $('order-rows').replaceChildren(
    ...orderFolders.map((folder) => {
      const row = mods.find((m) => m.folder === folder);
      const tr = document.createElement('tr');
      tr.dataset.folder = folder;
      tr.append(cell(row ? row.name : folder), cell(row ? row.state : ''));
      tr.addEventListener('click', () => selectOrder(folder));
      return tr;
    })
  );
  selectOrder(selectedOrder);
}

function moveOrder(delta) {
  const i = orderFolders.indexOf(selectedOrder);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= orderFolders.length) return;
  [orderFolders[i], orderFolders[j]] = [orderFolders[j], orderFolders[i]];
  orderDirty = true;
  renderOrder();
}

// mods, die vor einer ihrer abhängigkeiten stehen
function orderProblems() {
  const problems = [];
  orderFolders.forEach((folder, i) => {
    const extra = extras.mods.find((e) => e.folder === folder);
    if (!extra) return;
    for (const dep of extra.deps) {
      const depExtra = extras.mods.find((e) => e.id === dep);
      if (!depExtra) continue;
      if (orderFolders.indexOf(depExtra.folder) > i) {
        problems.push(`${modNameFor(folder)} loads before ${modNameFor(depExtra.folder)}, which it needs`);
      }
    }
  });
  return problems;
}

async function saveOrder() {
  const problems = orderProblems();
  if (problems.length && !(await askConfirm(`${problems.join('\n')}\n\nSave anyway?`))) return;
  try {
    await invoke('save_load_order', { order: orderFolders });
    orderDirty = false;
    updateOrderButtons();
    setStatus('Load order saved. It applies the next time Horsey starts.');
  } catch (err) {
    setStatus(String(err));
  }
}

$('btn-order-up').addEventListener('click', () => moveOrder(-1));
$('btn-order-down').addEventListener('click', () => moveOrder(1));
$('btn-order-revert').addEventListener('click', refreshOrder);
$('btn-order-save').addEventListener('click', saveOrder);
