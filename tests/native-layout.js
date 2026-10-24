// Actual WebView regression: fixed viewport, integrated controls, focus extraction and update privacy.
(async () => {
  const invoke = (cmd, args = {}) => window.__TAURI_INTERNALS__.invoke(cmd, args);
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const assert = (ok, message) => { if (!ok) throw Error(message); };
  const checks = [];
  try {
    for (let i = 0; i < 100 && !document.querySelector('.meeting-link'); i++) await sleep(100);
    document.querySelector('.meeting-link').click();
    await sleep(1500);
    const header = document.querySelector('.meeting-header');
    assert(header && header.getBoundingClientRect().top < 20, 'Meeting header must be integrated into the top of the content');
    assert(document.querySelector('.window-toolbar [data-panel="sidebar"]'), 'Sidebar toggle missing at top');
    document.querySelector('[data-action="workspace-view"]').click();
    for (const panel of ['notebook','minutes','transcript']) {
      let button = document.querySelector(`#view-menu [data-panel="${panel}"]`);
      assert(button, 'Missing panel toggle ' + panel);
      const prior = button.getAttribute('aria-expanded');
      button.click(); await sleep(350);
      button = document.querySelector(`#view-menu [data-panel="${panel}"]`);
      assert(button.getAttribute('aria-expanded') !== prior, 'Panel did not toggle ' + panel);
      button.click(); await sleep(350);
    }
    document.querySelector('[data-action="workspace-view"]').click();
    assert(!document.querySelector('.columns .panel-rail'), 'Collapsed columns must not leave a confusing rail');
    assert(!document.querySelector('.session-strip [data-action="toggle-transcript"]'), 'Duplicated view controls remain');
    for (const selector of ['#meeting-title', '#record', '[data-action="workspace-view"]']) {
      const element = document.querySelector(selector), rect = element.getBoundingClientRect();
      assert(rect.left >= 0 && rect.right <= innerWidth, 'Top control outside viewport: ' + selector);
      assert(element.contains(document.elementFromPoint(rect.x + rect.width/2, rect.y + rect.height/2)), 'Top control obscured: ' + selector);
    }
    checks.push('Integrated meeting actions and all panel toggles are visible and clickable');
    const footer = document.querySelector('#workspace footer');
    const before = footer.getBoundingClientRect().bottom;
    window.scrollTo(0, 10000);
    document.documentElement.scrollTop = 10000;
    document.body.scrollTop = 10000;
    await sleep(100);
    assert(window.scrollY === 0 && document.documentElement.scrollTop === 0, 'Outer window scrolled');
    assert(Math.abs(footer.getBoundingClientRect().bottom - before) < 1, 'Footer moved with outer scroll');
    assert(Math.abs(before - innerHeight) < 2, 'White gap below footer');
    assert(getComputedStyle(document.body).overscrollBehaviorY === 'none', 'Root overscroll enabled');
    checks.push('Outer viewport fixed and footer meets window edge');
    const terms = await invoke('extract_focus_terms', {content:'我正在测试软件的会议纪要功能，但是没有自动提取关键词，内容重复了一遍。'});
    assert(terms.length > 0 && terms.every(x => x.length < 12), 'Focus copied prose');
    checks.push('Focus terms are local keywords, not copied sentences');
    await invoke('set_network_policy', {policy:{allow_model_downloads:false,allow_online_recommendations:false,allow_update_checks:false}});
    try { await invoke('check_update'); throw Error('Update check bypassed permission'); }
    catch(e) { assert(String(e).includes('network-disabled'), String(e)); }
    checks.push('Update requests are denied before network opt-in');
    document.dispatchEvent(new CustomEvent('open-settings', {detail:'updates'}));
    await sleep(1200);
    assert(document.querySelector('#updates-network'), 'Update settings missing');
    assert(document.querySelector('[data-update="check_update"]').disabled, 'Check enabled without consent');
    checks.push('Update settings explain and enforce opt-in');
    document.querySelector('[data-settings="back"]').click();
    await sleep(500);
    await invoke('ui_test_report', {report:{passed:true, checks, viewport:{width:innerWidth,height:innerHeight}}});
  } catch(e) { await invoke('ui_test_report', {report:{passed:false,error:String(e),checks}}); }
})();
