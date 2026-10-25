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
 check(title.getBoundingClientRect().width<document.querySelector('.meeting-header').clientWidth*.8,'Title fills toolbar');title.blur();
 check(document.querySelector('.window-toolbar').getBoundingClientRect().height===44,'Title bar too tall');
 check(document.querySelector('#workspace-dock').getBoundingClientRect().top<100,'Too much chrome above content');
 check(!document.querySelector('footer'),'Footer returns');
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
 await invoke('set_view_settings',{preferences:{...await invoke('get_view_settings'),companion_enabled:true}});await sleep(250);
 check((await invoke('ui_test_windows')).find(w=>w.label==='companion').visible,'Companion cannot be restored');
 checks.push('Desktop pet has a separate transparent surface; controls expand and the visibility preference persists');
 await invoke('ui_test_report',{report:{passed:true,checks}});
 await invoke('set_companion_expanded',{expanded:true});
}catch(e){await invoke('ui_test_report',{report:{passed:false,error:String(e),checks,diagnostics:{mode:document.querySelector(".transcript-toggle")?.getAttribute("aria-pressed"),floating:document.querySelectorAll(".dv-resize-container").length,transcriptParent:document.querySelector(".transcript-pane")?.parentElement?.outerHTML.slice(0,400),visibleText:document.querySelector(".transcript-pane")?.innerText.slice(0,160)}}})}
})();
