// Native UI/IPC smoke test. Use an isolated OMASCRIBE_DATA_DIR with ui-test enabled.
(async () => {
  const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
  const until = async test => { const end = Date.now() + 15000; while (Date.now() < end) { if (await test()) return; await new Promise(r => setTimeout(r, 100)); } throw Error(`Timed out: ${test}`); };
  const click = q => { const el = document.querySelector(q); if (!el) throw Error(`Missing ${q}`); el.click(); };
  const checks = [];
  try {
    await until(() => document.querySelector('[data-action="settings"]'));
    await invoke('set_network_policy', {policy: {allow_model_downloads: false, allow_online_recommendations: false}});
    const before = await invoke('get_preferences');
    for (const command of ['download_model', 'open_configuration_gallery']) {
      let blocked = false;
      try { await invoke(command, command === 'download_model' ? {id:'qwen3-4b'} : {}); } catch (error) { blocked = String(error).startsWith('network:'); }
      if (!blocked) throw Error(`${command} bypassed offline permissions`);
    }
    checks.push('Downloads and online gallery blocked by default without changing inference preferences');
    if (JSON.stringify(before) !== JSON.stringify(await invoke('get_preferences'))) throw Error('Online action changed inference preferences');
    click('[data-action="settings"]');
    await until(() => document.querySelector('[data-section="privacy"]'));
    click('[data-section="privacy"]');
    await until(() => document.querySelector('[data-network="allow_model_downloads"]'));
    click('[data-network="allow_model_downloads"]');
    await until(async () => (await invoke('get_network_policy')).allow_model_downloads);
    if ((await invoke('get_network_policy')).allow_online_recommendations) throw Error('Permissions are coupled');
    click('[data-network="allow_model_downloads"]');
    await until(async () => !(await invoke('get_network_policy')).allow_model_downloads);
    checks.push('Separate network switches persist through native UI and return to offline');
    click('[data-section="models"]');
    await until(() => document.querySelector('[data-pref="inference_backend"]'));
    const hardware = await invoke('hardware_profile');
    if (!hardware.resource_presets.length || !hardware.backends.includes('cpu')) throw Error('Missing backend/preset inventory');
    click('[data-preset="eco"]');
    await until(async () => (await invoke('get_preferences')).resource_mode === 'eco');
    await invoke('set_preferences', {preferences: before});
    checks.push('Resource preset reaches backend preferences; original test configuration restored');
    click('[data-section="privacy"]');
    await invoke('ui_test_report', {report: {ok:true, checks, hardware}});
  } catch (error) { await invoke('ui_test_report', {report: {ok:false, error:String(error), checks}}); }
})();
