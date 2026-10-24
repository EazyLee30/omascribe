// Run with ui-test feature and an isolated OMASCRIBE_DATA_DIR. No microphone access.
(async () => {
  const invoke = (command, args = {}) => window.__TAURI_INTERNALS__.invoke(command, args);
  const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));
  const until = async predicate => { const end=Date.now()+15000;while(Date.now()<end){if(await predicate())return;await sleep(100);}throw Error('Timed out: '+predicate); };
  const click = query => {const node=document.querySelector(query);if(!node)throw Error('Missing '+query);node.click();};
  const input = (query,value) => {const node=document.querySelector(query);node.value=value;node.dispatchEvent(new Event('input',{bubbles:true}));};
  const checks=[];
  try {
    await until(()=>document.querySelector('[data-action="settings"]'));
    click('[data-action="settings"]');await until(()=>document.querySelector('[data-mode="light"]'));
    click('[data-mode="light"]');await until(async()=>(await invoke('get_preferences')).appearance.mode==='light');
    checks.push('Appearance changes persist through native IPC');
    await until(()=>document.querySelector('[data-section="models"]'));click('[data-section="models"]');await until(()=>document.querySelector('#model-list'));
    const hardware=await invoke('hardware_profile');if(!hardware.total_memory_mib||!hardware.cores)throw Error('Hardware data missing');
    const models=await invoke('model_status');if(!models.some(m=>m.installed))throw Error('Real installed models not detected');
    checks.push('Hardware and installed models read from this Mac');
    click('#sidebar-content [data-page="notes"]');await until(()=>document.querySelector('[data-library="new"]'));
    click('[data-library="new"]');await until(()=>document.querySelector('#entry-title'));
    input('#entry-title','Native workspace validation');input('#entry-content','Project Owl uses offline transcription. This is a fictional validation note.');
    const knowledge=document.querySelector('#entry-knowledge');knowledge.checked=true;knowledge.dispatchEvent(new Event('input',{bubbles:true}));
    click('#sidebar-content [data-page="knowledge"]');await until(()=>document.querySelector('.library-entry'));
    const entries=await invoke('list_library');if(!entries.some(e=>e.title==='Native workspace validation'&&e.knowledge&&e.content.includes('Project Owl')))throw Error('Library autosave did not persist');
    checks.push('Immediate navigation flushes a note and its knowledge opt-in to SQLite');
    const meeting=(await invoke('list_meetings')).find(m=>Object.keys(m.speaker_names||{}).length);
    if(meeting){
      const before=await invoke('snapshot',{id:meeting.id});const id=Object.keys(meeting.speaker_names)[0];
      await invoke('rename_speaker',{meeting:meeting.id,id,name:'测试发言人'});
      const after=await invoke('snapshot',{id:meeting.id});
      if(after.meeting.speaker_names[id]!=='测试发言人'||!after.segments.some((s,i)=>s.revision>before.segments[i].revision))throw Error('Speaker revision missing');
      checks.push('Renaming a speaker persists and revises source passages');
    }
    click('[data-action="settings"]');await until(()=>document.querySelector('[data-section="appearance"]'));
    await invoke('ui_test_report',{report:{ok:true,checks,hardware,modelCount:models.length}});
  } catch(error) {await invoke('ui_test_report',{report:{ok:false,error:String(error),checks}});}
})();
