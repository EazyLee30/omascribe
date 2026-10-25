// End-to-end layout review in the installed native WebView, on fictional data only.
(async()=>{
const invoke=(c,a={})=>window.__TAURI_INTERNALS__.invoke(c,a),sleep=ms=>new Promise(r=>setTimeout(r,ms));
const check=(v,m)=>{if(!v)throw Error(m)},checks=[];
const until=async f=>{for(let i=0;i<150;i++){if(await f())return;await sleep(100)}throw Error('Timed out')};
try{
 await until(()=>document.querySelector('.meeting-link'));await sleep(800);
 for(const m of await invoke('list_meetings')){if((await invoke('snapshot',{id:m.id})).segments.length){document.querySelector(`.meeting-link[data-meeting="${m.id}"]`).click();await sleep(800);break}}
 await invoke("ui_test_report",{report:{running:true}});
 document.querySelector('[data-action="workspace-view"]').click();document.querySelector('[data-layout="default"]').click();await sleep(600);
 check(document.querySelector('#workspace-dock .dv-dockview'),'Docking library missing');
 check(document.querySelectorAll('.meeting-header').length===1,'Duplicate meeting toolbar');
 const title=document.querySelector('#meeting-title');title.focus();check(getComputedStyle(title).outlineStyle==='none','Long title focus frame');
 check(!document.querySelector('#sync'),'Redundant persistent save label returned');
 check(title.getBoundingClientRect().right<=document.querySelector('.recording-console').getBoundingClientRect().left+1,'Toolbar actions overlap the meeting title');
 check(title.getBoundingClientRect().width<document.querySelector('.meeting-header').clientWidth*.8,'Title fills toolbar');title.blur();
 check(document.querySelector('.window-toolbar').getBoundingClientRect().height===44,'Title bar too tall');
 check(document.querySelector('#workspace-dock').getBoundingClientRect().top<100,'Too much chrome above content');
 check(!document.querySelector('footer'),'Footer returns');
 for(const e of [document.documentElement,document.body,document.querySelector('#workspace'),...document.querySelectorAll('.oma-dock-theme .dv-view')]){
   check(e.scrollWidth<=e.clientWidth+1&&e.scrollHeight<=e.clientHeight+1,'An outer layout container overflowed: '+(e.id||e.className));
 }
 check(document.querySelector('.meeting-header .recording-console'),'Live controls were not moved to the title row');
 check(document.querySelector('.selection-toolbar').hidden,'Fixed formatting toolbar returned');
 check(getComputedStyle(document.querySelector('#user-notes')).borderTopWidth==='0px','Framed editor returned');
 for(const q of ['.transcript-toggle','.caption-toggle','#record']){const e=document.querySelector(q),r=e.getBoundingClientRect();check(r.width>0&&e.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)),'Live control hidden or covered: '+q)}
 checks.push('Quiet title, browser-style meeting tab, compact chrome, direct recording/transcript/caption console');
 const transcript=document.querySelector('.transcript-toggle');transcript.click();await until(async()=> (await invoke('ui_test_windows')).some(w=>w.label==='transcript'&&w.visible));
 const windows=await invoke('ui_test_windows');check(windows.some(w=>w.label==='main')&&windows.some(w=>w.label==='transcript'),'Transcript is not a separate OS window');
 const views=await invoke('get_view_settings');await invoke('set_view_settings',{preferences:{...views,transcript_mode:'docked'}});await sleep(700);
 check(document.querySelector('#translation-mode').options.length===4,'Translation choices missing');
 check(document.querySelectorAll('.transcript-pane .segment').length>0,'Opening transcript did not reveal existing text');
 check(document.querySelectorAll('#workspace-dock .dv-tab').length===3,'Transcript failed to dock');
 checks.push('One click opens a separate OS transcript window; docking restores the main panel and language choices');
 document.querySelector('[data-action="workspace-view"]').click();document.querySelector('[data-action="save-layout"]').click();await until(()=>document.querySelector('#layout-name'));
 const input=document.querySelector('#layout-name');input.value='Review layout';document.querySelector('[data-action="confirm-save-layout"]').click();await sleep(300);
 document.querySelector('[data-action="workspace-view"]').click();const saved=document.querySelector('[data-saved-layout]');check(saved,'Custom layout not saved');const savedId=saved.dataset.savedLayout;
 document.querySelector('[data-layout="notes"]').click();await sleep(500);
 check(document.querySelectorAll('#workspace-dock .dv-tab').length===1,'Focus preset did not simplify layout');
 document.querySelector('[data-action="workspace-view"]').click();document.querySelector(`[data-saved-layout="${savedId}"]`).click();await sleep(500);
 check(document.querySelectorAll('#workspace-dock .dv-tab').length===3,'Saved split layout not restored');
 checks.push('Named layout save, focus preset and full layout restoration retain panels');
 const before=(await invoke('list_meetings')).length;
 const tabsBefore=document.querySelectorAll('.meeting-tab').length;
 document.querySelector('[data-action="new-tab"]').click();await until(()=>document.querySelector('.tab-picker'));
 document.querySelector('#modal [data-action="new"]').click();await sleep(1000);
 check(document.querySelectorAll('.meeting-tab').length===tabsBefore+1,'New meeting did not open in a tab');
 const active=document.querySelector('.meeting-tab.active [data-close-tab]');active.click();await sleep(1000);
 check((await invoke('list_meetings')).length===before+1,'Closing a tab deleted the meeting');
 check(document.querySelectorAll('.meeting-tab').length===tabsBefore,'Tab did not close');
 checks.push('Meeting tabs switch independently; closing a tab keeps the meeting');
 document.querySelector('[data-action="workspace-view"]').click();document.querySelector('[data-layout="default"]').click();await sleep(400);
 await invoke('ui_test_resize',{width:740,height:560});await sleep(700);
 for(const q of ['.transcript-toggle','.caption-toggle','#record','[data-action="workspace-view"]']){const e=document.querySelector(q),r=e.getBoundingClientRect();check(r.left>=0&&r.right<=innerWidth&&r.bottom<=innerHeight,'Control outside small window: '+q);check(e.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)),'Small window control covered: '+q)}
 check(document.documentElement.scrollWidth<=innerWidth,'Window has horizontal overflow');
 checks.push('740 × 560 viewport keeps every live control visible');
 await invoke('ui_test_resize',{width:1160,height:780});await sleep(400);
 const prefs=await invoke('get_view_settings');
 await invoke('set_view_settings',{preferences:{...prefs,companion_enabled:true}});await sleep(400);
 const pet=(await invoke('ui_test_windows')).find(w=>w.label==='companion'&&w.visible);check(pet,'Desktop companion is missing');
 await invoke('set_companion_expanded',{expanded:true});await sleep(300);
 const expandedPet=(await invoke('ui_test_windows')).find(w=>w.label==='companion');check(expandedPet.size[0]>pet.size[0]*2,'Companion controls did not expand');
 await invoke('set_companion_expanded',{expanded:false});
 await invoke('set_view_settings',{preferences:{...await invoke('get_view_settings'),companion_enabled:false}});await sleep(250);
 check(!(await invoke('ui_test_windows')).find(w=>w.label==='companion').visible,'Companion hide preference failed');
 const restore=document.querySelector('.sidebar-bottom [data-action="toggle-companion"]');check(restore,'Pet restore entry missing from the sidebar');restore.click();await sleep(500);
 check((await invoke('ui_test_windows')).find(w=>w.label==='companion').visible,'Companion cannot be restored');
 await invoke('show_main_window');await sleep(250);restore.focus();await sleep(250);
 check(Number(getComputedStyle(restore.querySelector('.pet-toggle-hint')).opacity)>0.9,'Pet hover/focus action is not visible');
 check(restore.getAttribute('aria-pressed')==='true'&&/Hide|隐藏|隱藏/.test(restore.querySelector('.pet-toggle-hint').textContent),'Pet action did not change to Hide');
 restore.click();await sleep(500);
 check(!(await invoke('ui_test_windows')).find(w=>w.label==='companion').visible,'Sidebar Hide did not hide the pet');
 check(/Show|显示|顯示/.test(restore.querySelector('.pet-toggle-hint').textContent),'Hidden pet action did not change to Show');
 restore.click();await sleep(500);
 check((await invoke('ui_test_windows')).find(w=>w.label==='companion').visible,'Sidebar Show failed to restore pet');
 checks.push('Desktop pet has a separate transparent surface; controls expand, hover/focus reveals Show or Hide, and the sidebar toggles visibility');
 if(/Mac/.test(navigator.platform)){
   await invoke('show_main_window');
   const main=async()=>(await invoke('ui_test_windows')).find(w=>w.label==='main');
   const before=await main();
   document.querySelector('.window-toolbar').dispatchEvent(new MouseEvent('dblclick',{bubbles:true,button:0}));
   await until(async()=> (await main()).maximized!==before.maximized);
   document.querySelector('.window-toolbar').dispatchEvent(new MouseEvent('dblclick',{bubbles:true,button:0}));
   await until(async()=> (await main()).maximized===before.maximized);
   const restored=await main();check(restored.size.every((v,i)=>Math.abs(v-before.size[i])<=2),'Double click did not restore the prior window dimensions');
   check(document.querySelectorAll('[data-window-resize]').length===8,'Window edge or corner resize grip missing');
   const edge=document.querySelector('[data-window-resize="se"]');
   edge.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerId:91,button:0,screenX:100,screenY:100}));await sleep(150);
   edge.dispatchEvent(new PointerEvent('pointermove',{bubbles:true,pointerId:91,screenX:160,screenY:130}));await sleep(200);
   edge.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,pointerId:91,button:0,screenX:160,screenY:130}));await sleep(200);
   const resized=await main();check(resized.size[0]>before.size[0]&&resized.size[1]>before.size[1],'Corner gesture did not resize the native window');
   edge.dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,pointerId:92,button:0,screenX:100,screenY:100}));await sleep(150);
   edge.dispatchEvent(new PointerEvent('pointermove',{bubbles:true,pointerId:92,screenX:-2000,screenY:-2000}));await sleep(200);
   edge.dispatchEvent(new PointerEvent('pointerup',{bubbles:true,pointerId:92,button:0,screenX:-2000,screenY:-2000}));await sleep(200);
   const minimum=await main();check(minimum.size[0]>=740*devicePixelRatio&&minimum.size[1]>=560*devicePixelRatio,'Resize broke the minimum window size');
   await invoke('ui_test_resize',{width:1160,height:780});
   checks.push('Titlebar double click maximizes/restores; all eight edge grips exist and a corner pointer gesture changes the native window size');
 }
 await invoke('ui_test_report',{report:{passed:true,checks}});
}catch(e){await invoke('ui_test_report',{report:{passed:false,error:String(e),checks,diagnostics:{mode:document.querySelector(".transcript-toggle")?.getAttribute("aria-pressed"),floating:document.querySelectorAll(".dv-resize-container").length,transcriptParent:document.querySelector(".transcript-pane")?.parentElement?.outerHTML.slice(0,400),visibleText:document.querySelector(".transcript-pane")?.innerText.slice(0,160)}}})}
})();
