import {writeSync} from 'node:fs';
import {readFile, writeFile, readdir, rename, open} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
import {setTimeout as wait} from 'node:timers/promises';
import {nativeClock} from './native-clock.mjs';
import {AsyncLocalStorage} from 'node:async_hooks';

const help=`Usage: node scripts/game.mjs CONFIG.json REQUEST.json
Start a new isolated prepared server with ./run.sh PORT --test-state NEW_DIRECTORY --no-shore-bots.
CONFIG: {"origin":"http://127.0.0.1:PORT","state":"/absolute/NEW_DIRECTORY"}
REQUEST: {"id":"unique-stable-id","command":{"op":"capabilities"}}
Reuse the same id only to retrieve the same command's result, never to retry uncertain effects.
Read operations: capabilities; catalog {search,limit}; world_info {world}; observe {actor}; inspect {actor}; native_state {actors}; result {request}; trace {request,offset,limit}.
Spawn: {op:"spawn",spec:{guid,class,level,seed,equipment:[weapon,ability,armor,ring],backpack:[eight explicit slots],world,position:{x,y},controller:"direct"|"native"|"game_bot",profile:null|"Pixl",stats:null|[eight explicit stats]}}. Empty slot is -1. IDs and names come from catalog. A dead prepared actor may be replaced on the same account; a living actor is never implicitly reset. Spawn never chooses class, level, gear or a world for you. game_bot also requires an exact advertised profile; other controllers require no profile.
Direct act: {op:"act",actor,action:{command:...},duration_ms,pauseAfter:true} uses ordinary server actions. run {actor,goal,max_game_ms,pauseAfter:true} delegates only advertised goals to the actual gameplay bot; unsupported classes/goals fail.
Native CONFIG also requires browserTools:"/absolute/managed/direct-cdp.mjs" and native:[{port,target,actor}] for EVERY connected native actor. Native act uses input:{keys:["d"],duration_ms:600,mouse:{x:450,y:300}}; mouse is optional left firing in the unclipped 800x600 client. Keys and durations are bounded. Native inventory drag uses input:{keys:[],duration_ms:350,drag:{from:{x:634,y:528},to:{x:634,y:428}}}. expect:{world,items:{slot:item},inventory_revision} asserts outcomes; require:{world,inventory_revision} checks starting state. No desktop activation.
pauseAfter defaults to true: the whole isolated session does not progress while you inspect and think. Native pause requires Ruffle suspension, pause-adjusted page clocks and server pause; simulation-only pause is not native acceptance. Initialize while Ruffle is playing; an earlier untracked pause requires normal reconnection of the same living character. Native operations cannot return unpaused: capture during a bounded action. A lost controller's 1500ms lease stops the prepared simulation and closes native connections; expiry makes that session terminal.
Starting context only: prepare_realm_capacity {limit} lowers only the active Realm once, before any enrollment/ownership claims, with the prepared clock paused and 1 <= limit < global capacity. Normal portal publication and guarded admission remain authoritative; capabilities returns the prepared receipt. Other worlds/global capacity are unchanged.
Explicit paused mutations: world {template}; cheat {actor,change:{kind:"stats",values}|{kind:"item",slot,item}|{kind:"teleport",position}|{kind:"stat_modifier",index,amount,duration_ms}|{kind:"protection",condition,duration_ms}|{kind:"guild",name,members:[actorIds]}|{kind:"pause",paused:boolean}}. Protection permits invincible, stasis or invulnerable for 1–60000ms, with no overlapping active protection. Stat modifiers require panel index2–7, nonzero amount -100–100, duration1–60000ms and no active prepared modifier; base stats and inventory are unchanged. Guild setup creates a real, unearned relation for the selected founder and listed registered actors without changing wallets or stars; normal guild eligibility is unchanged. Guild client publication is deferred until the next ordinary unpaused tick (for native clients, use a normal bounded native action); paused success confirms the persisted/server relation, not rendered client state. Actor pause requires a direct actor and a changed desired state; it invokes ordinary /pause safety policy, without advancing time or changing guild membership. Actor pause status publication also waits for the next ordinary unpaused tick; the immediate ordinary chat reply is not client status proof. These are recorded cheats, never earned gameplay.
A reached time window is not a dungeon clear. Inspect actual effects and goal_reached. cancel {request} addresses a running server request by stable id. Compact receipts point to bounded evidence in STATE/testing; preserve failed receipts and dead characters.
`;
const [configPath, commandPath] = process.argv.slice(2);
if(configPath==='--help'&&process.argv.length===3){console.log(help);process.exit(0);}
if (!configPath || !commandPath || process.argv.length !== 4) throw new Error(help);
const config = JSON.parse(await readFile(configPath, 'utf8'));
const {id, command} = JSON.parse(await readFile(commandPath, 'utf8'));
if(typeof id!=='string'||!id.length||id.length>64||!/^[A-Za-z0-9_-]+$/.test(id)||!command||typeof command!=='object') throw new Error('Command file requires {id,command}; id must be 1–64 ASCII letters, digits, hyphens or underscores and remain stable on retry');
const origin = new URL(config.origin);
if (origin.protocol !== 'http:' || !['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname) || origin.pathname !== '/' || origin.search || origin.hash) throw new Error('Testing requires an explicit loopback HTTP origin');
const token = (await readFile(`${config.state}/control.token`, 'utf8')).trim();
const marker=JSON.parse(await readFile(`${config.state}/testing.json`,'utf8'));
if(marker.schema!==1||marker.provenance!=='prepared') throw new Error('Native bridge requires an explicitly prepared test state');
const path=`${config.state}/testing/${id}.client.json`;
const mutating=!['capabilities','catalog','world_info','observe','inspect','result','trace','native_state'].includes(command.op);
const syncDirectory=async()=>{const dir=await open(`${config.state}/testing`,'r');try{await dir.sync();}finally{await dir.close();}};
let ownsIntent=false, mayProgress=false, sequence=0;
if(mutating) {
  let saved;
  try {saved=JSON.parse(await readFile(path,'utf8'));} catch(e){if(e.code!=='ENOENT')throw e;}
  if(saved) {
    if(JSON.stringify(saved.command)!==JSON.stringify(command))throw new Error('Request id was already used for a different command');
    if(saved.status==='running'){console.error(JSON.stringify({id,status:'uncertain',error:'Request has no confirmed result; inspect server trace and actor state instead of replaying effects'}));process.exit(1);}
    console.log(JSON.stringify({...saved.result,evidence:path}));process.exit(saved.result.status==='failed'?1:0);
  }
  if((await readdir(`${config.state}/testing`)).filter(n=>n.endsWith('.client.json')).length>=256)throw new Error('Native receipt budget reached; preserve this evidence and create a new owned test state');
  await writeFile(path,JSON.stringify({command,status:'running'}),{flag:'wx',mode:0o600,flush:true});ownsIntent=true;await syncDirectory();
}
const save=async result=>{if(ownsIntent){await writeFile(`${path}.tmp`,JSON.stringify({command,status:'complete',result}),{flag:'wx',mode:0o600,flush:true});await rename(`${path}.tmp`,path);await syncDirectory();}};
const api = async (value, requestId=`${id}-ctl${++sequence}`) => {
  const started=performance.now();
  try {
    const response = await fetch(new URL('/dev/testing', origin), {
      method: 'POST', headers: {Authorization:`Bearer ${token}`, 'Content-Type':'application/json'},
      body: JSON.stringify({id:requestId, command:value}),
      signal: AbortSignal.timeout(value.op==='run'?660000:value.op==='native_sync'?45000:value.op==='native_lease'&&value.operation==='renew'?500:10000),
    });
    const result = await response.json();
    if (!response.ok || result.status === 'failed') throw new Error(JSON.stringify(result));
    return result;
  } finally {console.error(JSON.stringify({event:'native_http_timing',op:value.op,wall_ms:performance.now()-started}));}
};
const clients = [];
async function eachClient(selected, action) {
  const results=await Promise.allSettled(selected.map(action));
  for(const result of results)if(result.status==='rejected')throw result.reason;
  return results.map(result=>result.value);
}
async function connect(spec) {
  if (!config.browserTools) throw new Error('Native control requires the installed managed browser helper path');
  if(!Number.isInteger(spec.port)||spec.port<1024||spec.port>65535||typeof spec.target!=='string'||!Number.isInteger(spec.actor))throw new Error('Each native target requires an explicit local port, target id and durable actor id');
  const {managedCDP, scriptOwner} = await import(pathToFileURL(config.browserTools).href);
  let send;const spans=new AsyncLocalStorage();
  const managed=await managedCDP(spec.port,spec.target,scriptOwner(),(method,params)=>{
    const span=spans.getStore();if(span)span.rawStart=performance.now();
    return send(method,params).finally(()=>{if(span)span.rawEnd=performance.now();});
  });
  const call=(method,params={})=>{
    const span={start:performance.now()};
    return spans.run(span,async()=>{try{return await managed(method,params);}finally{
      const end=performance.now();console.error(JSON.stringify({event:'native_cdp_timing',method,wall_ms:end-span.start,before_raw_ms:span.rawStart===undefined?null:span.rawStart-span.start,raw_roundtrip_ms:span.rawEnd===undefined?null:span.rawEnd-span.rawStart,after_raw_ms:span.rawEnd===undefined?null:end-span.rawEnd}));
    }});
  };
  const pages = await (await fetch(`http://127.0.0.1:${spec.port}/json/list`,{signal:AbortSignal.timeout(5000)})).json();
  const page = pages.find(p => p.id === spec.target);
  if (!page || new URL(page.url).origin !== origin.origin) throw new Error(`Native target ${spec.target} is missing or on the wrong origin`);
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve,reject) => {const timeout=setTimeout(()=>{ws.close();reject(new Error('CDP connection deadline'));},10000);ws.onopen=()=>{clearTimeout(timeout);resolve();};ws.onerror=e=>{clearTimeout(timeout);reject(e);};});
  const pending = new Map(), frames=[]; let serial = 0, holdState, closing=false;
  ws.onmessage = event => {
    const row=JSON.parse(event.data);
    if(row.method==='Network.webSocketFrameReceived'&&row.params.response.opcode===2){const frame=Buffer.from(row.params.response.payloadData,'base64');if(frame.length===9&&frame.readUInt32BE(0)===9&&frame[4]===6){frames.push(createHash('sha256').update(frame).digest('hex'));if(frames.length>128)frames.shift();}}
    const item=pending.get(row.id);
    if (!item) return;
    pending.delete(row.id);clearTimeout(item.timer);
    if (row.error) item.reject(new Error(JSON.stringify(row.error))); else item.resolve(row.result);
  };
  ws.onclose=()=>{const error=new Error('Native target connection closed');if(!closing){leaseError=error;holdState?.abort(error);}for(const item of pending.values()){clearTimeout(item.timer);item.reject(error);}pending.clear();};
  send = (method, params) => new Promise((resolve,reject) => {
    if(method.startsWith('Input.')&&['keyDown','mousePressed'].includes(params.type)){
      if(leaseError){holdState?.abort(leaseError);reject(leaseError);return;}
      if(!holdState){reject(new Error('Native DOWN outside its bounded input'));return;}
      if(holdState.started&&Date.now()-holdState.started>=holdState.ms){reject(new Error('Native DOWN missed its release deadline'));return;}
      if(!holdState.started)holdState.start();
    }
    const n=++serial, timer=setTimeout(() => {pending.delete(n);reject(new Error(`CDP deadline: ${method}`));},10000);
    pending.set(n,{resolve,reject,timer}); ws.send(JSON.stringify({id:n,method,params}));
  });
  const evaluate=async expression => {
    const r=await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});
    if(r.exceptionDetails) throw new Error(JSON.stringify(r.exceptionDetails));
    return r.result.value;
  };
  const clock=paused=>evaluate(`(${nativeClock.toString()})(${JSON.stringify(paused)})`);
  async function hold(keys,mouse,ms,dragTo){
    if(holdState)throw Error('Concurrent native hold is forbidden');
    const key=(type,k)=>call('Input.dispatchKeyEvent',{type,key:k,code:bindings[k][0],windowsVirtualKeyCode:bindings[k][1]});
    let resolveRelease,rejectRelease,timer,dragTimer,releasedAt;
    const released=new Promise((ok,bad)=>{resolveRelease=ok;rejectRelease=bad;});
    const release=async()=>{releasedAt=Date.now();const results=await Promise.allSettled([...keys.map(k=>key('keyUp',k)),...(mouse?[call('Input.dispatchMouseEvent',{type:'mouseReleased',...(dragTo??mouse),button:'left',buttons:0,clickCount:1})]:[])]);const errors=results.filter(r=>r.status==='rejected').map(r=>String(r.reason));if(errors.length)throw Error(errors.join('; '));};
    holdState={ms,started:0,abort(error){clearTimeout(timer);clearTimeout(dragTimer);release().then(()=>rejectRelease(error),failure=>rejectRelease(Error(String(error)+'; input release unconfirmed: '+failure)));},start(){this.started=Date.now();timer=setTimeout(()=>release().then(resolveRelease,rejectRelease),ms);if(dragTo)dragTimer=setTimeout(()=>call('Input.dispatchMouseEvent',{type:'mouseMoved',...dragTo,button:'left',buttons:1}).catch(error=>this.abort(error)),ms/2);}};
    // Ruffle's map click establishes Flash focus before movement keys are delivered.
    const down=(async()=>{const results=mouse?await Promise.allSettled([call('Input.dispatchMouseEvent',{type:'mousePressed',...mouse,button:'left',buttons:1,clickCount:1})]):[];if(results.every(r=>r.status==='fulfilled'))results.push(...await Promise.allSettled(keys.map(k=>key('keyDown',k))));return results;})();
    if(!keys.length&&!mouse)holdState.start();
    const noDispatch=setTimeout(()=>{if(!holdState.started)rejectRelease(Error('Native DOWN was not dispatched before its deadline'));},10000);
    try{await released;return {down,started:holdState.started,released:releasedAt};}finally{clearTimeout(timer);clearTimeout(dragTimer);clearTimeout(noDispatch);holdState=null;}
  }
  try { await call('Network.enable'); }
  catch(error) { closing=true;ws.close();throw error; }
  return {...spec,call,evaluate,clock,frames,hold,abort:error=>holdState?.abort(error),close:()=>{closing=true;ws.close();}};
}
let leaseStarted=false, leaseTimer, leaseRenewal, leaseError;
async function startLease(){
  if(leaseStarted)return;
  await api({op:'native_lease',owner:id,operation:'begin',actors:clients.map(c=>c.actor)});leaseStarted=true;
  leaseTimer=setInterval(()=>{
    if(leaseRenewal||leaseError)return;
    leaseRenewal=api({op:'native_lease',owner:id,operation:'renew'}).catch(e=>{leaseError=e;for(const c of clients)c.abort(e);}).finally(()=>{leaseRenewal=null;});
  },250);
}
async function endLease(){
  clearInterval(leaseTimer);if(leaseRenewal)await leaseRenewal;
  if(leaseError)throw leaseError;
  if(leaseStarted){await api({op:'native_lease',owner:id,operation:'end'});leaseStarted=false;}
}
async function abortLease(){
  clearInterval(leaseTimer);
  if(!leaseStarted)return;
  const state=await api({op:'native_state',actors:clients.map(c=>c.actor)});
  if(!state.stopped)await api({op:'native_lease',owner:id,operation:'halt'});
}
async function states(waitForReady=false,expect,previous){
  const deadline=Date.now()+5000;let state;
  do{
    if(leaseError)throw leaseError;
    state=await api({op:'native_state',actors:clients.map(c=>c.actor)});
    if(state.stopped)throw Error(state.stopped);
    if(state.actors.some(c=>['dying','dead','lost'].includes(c.stage)))throw Error('Native participant is terminal: '+JSON.stringify(state));
    const ready=state.actors.every(c=>c.stage==='ready'&&(!previous||c.player!==previous.actors.find(p=>p.actor===c.actor).player||c.client_time>previous.actors.find(p=>p.actor===c.actor).client_time));
    const expected=!expect||state.actors.find(c=>c.actor===command.actor)?.world_name===expect.world;
    if(ready&&expected)return state;
    if(!waitForReady)break;
    await wait(50);
  }while(Date.now()<deadline);
  throw Error('Native readiness/outcome not observed: '+JSON.stringify({expect,state}));
}
function verifiedNativeAck(context,server,frames,abort,write) {
  if(abort)throw abort;
  const observedBrowserFrameSha256=frames.find(frame=>frame===server.frame_sha256);
  if(server.actor!==context.actor||server.player!==context.player||!Number.isSafeInteger(server.nonce)||server.nonce<=-2147483648||server.nonce>=0||!observedBrowserFrameSha256)throw Error('No verified native_sync server/browser ACK');
  const row={event:'native_sync_verified_ack',context,server:{actor:server.actor,player:server.player,nonce:server.nonce,frame_sha256:server.frame_sha256},observedBrowserFrameSha256};
  const bytes=Buffer.from(JSON.stringify(row)+'\n');
  try{if(write(bytes)!==bytes.length)throw Error('Short native_sync ACK diagnostic write');}
  catch(cause){throw Error('Verified native_sync ACK diagnostic could not be retained',{cause});}
}
async function bind(state){
  const fresh=[];
  const pages=await eachClient(clients,async c=>({c,page:await c.evaluate('({playing:document.querySelector("ruffle-player").ruffle().isPlaying,binding:globalThis[Symbol.for("openrotmg.testing.binding.v1")]})')}));
  for(const {c,page} of pages){
    const actual=state.actors.find(a=>a.actor===c.actor);
    if(!page.playing){
      if(page.binding?.runtime!==state.runtime||page.binding.actor!==c.actor||page.binding.player!==actual.player)throw Error('Paused native target has no matching character/session binding');
    }else fresh.push(c);
  }
  if(fresh.length){
    let proof;
    try {proof=await api({op:'native_sync',actors:fresh.map(c=>c.actor)});}
    catch(error) {
      const context={cause:String(error),freshActors:fresh.map(c=>c.actor),sessions:state.actors.map(c=>({actor:c.actor,player:c.player,stage:c.stage,world:c.world})),pages:pages.map(({c,page})=>({actor:c.actor,playing:page.playing??null,bindingActor:page.binding?.actor??null,bindingPlayer:page.binding?.player??null,bindingRuntimeMatches:page.binding?page.binding.runtime===state.runtime:null,pingFrameCount:c.frames.length,pingFrameHashes:c.frames.slice(-8)}))};
      throw Error('native_sync failed: '+JSON.stringify(context),{cause:error});
    }
    await eachClient(fresh,async c=>{
      const acknowledgment=proof.acknowledged.find(a=>a.actor===c.actor),actual=state.actors.find(a=>a.actor===c.actor),deadline=Date.now()+1000;
      while(!c.frames.includes(acknowledgment.frame_sha256)&&Date.now()<deadline)await wait(10);
      if(actual.player!==acknowledgment.player||!c.frames.includes(acknowledgment.frame_sha256))throw Error('Native browser/character binding mismatch for actor '+c.actor);
      await c.evaluate('globalThis[Symbol.for("openrotmg.testing.binding.v1")]='+JSON.stringify({runtime:state.runtime,actor:c.actor,player:actual.player}));
      verifiedNativeAck({runtime:state.runtime,actor:c.actor,player:actual.player},acknowledgment,c.frames,leaseError,bytes=>writeSync(2,bytes));
    });
  }
}
async function initializeClock(c) {
  const status=await c.clock(null);
  if(typeof status?.initialized!=='boolean')throw Error('Native clock returned invalid initialization evidence');
}
async function pauseAll(expect) {
  if(!clients.length)return api({op:'pause',native_paused:[],native_sessions:[]});
  const state=await states(mayProgress,expect);
  await bind(state);
  await eachClient(clients,initializeClock);
  const confirmed=await eachClient(clients,async c=>{await c.clock(true);return c.actor;});
  const result=await api({op:'pause',native_paused:confirmed,native_sessions:state.actors.map(c=>[c.actor,c.player])});
  return {...result,pause_scope:'server_and_native'};
}
async function resumeAll() {
  if(!clients.length){mayProgress=true;await api({op:'resume'});return;}
  const before=await states();
  await bind(before);
  await eachClient(clients,initializeClock);
  await startLease();mayProgress=true;
  await api({op:'resume'});
  await eachClient(clients,c=>c.clock(false));
  // Focus can autoresume Ruffle; hidden members need it only after cohort clocks run.
  await eachClient(clients,c=>c.call('Emulation.setFocusEmulationEnabled',{enabled:true}));
  const after=await states(true,undefined,before);
  await bind(after);
  if(leaseError)throw leaseError;
}
const bindings = {
  w:['KeyW',87],a:['KeyA',65],s:['KeyS',83],d:['KeyD',68],
  ' ':['Space',32],Insert:['Insert',45],r:['KeyR',82],q:['KeyQ',81],e:['KeyE',69],i:['KeyI',73],
  '1':['Digit1',49],'2':['Digit2',50],'3':['Digit3',51],'4':['Digit4',52],
  '5':['Digit5',53],'6':['Digit6',54],'7':['Digit7',55],'8':['Digit8',56],
};
function nativeStep(actor,input){
  const client=clients.find(c=>c.actor===actor);
  if(!client)throw Error('Native act requires the configured owned target for this actor');
  if(!input||typeof input!=='object'||Object.keys(input).some(k=>!['duration_ms','keys','mouse','drag','target'].includes(k)))throw Error('Native input supports only duration_ms, keys and optional mouse or drag');
  if(!Number.isInteger(input.duration_ms)||input.duration_ms<1||input.duration_ms>10000)throw Error('Native duration_ms must be 1–10000');
  const keys=input.keys;
  if(!Array.isArray(keys)||new Set(keys).size!==keys.length||keys.some(k=>!Object.hasOwn(bindings,k)))throw Error('Native keys must be a unique array of supported keys');
  if(input.mouse&&(Object.keys(input.mouse).some(k=>!['x','y'].includes(k))||!Number.isFinite(input.mouse.x)||!Number.isFinite(input.mouse.y)||input.mouse.x<0||input.mouse.y<0||input.mouse.x>=800||input.mouse.y>=600))throw Error('Native mouse must be inside the 800×600 client');
  if(input.target&&(input.mouse||input.drag||!Number.isInteger(input.target.entity)||!Number.isFinite(input.target.camera_angle)||typeof input.target.fire!=='boolean'||Object.keys(input.target).some(k=>!['entity','camera_angle','fire'].includes(k))))throw Error('Native target requires entity, camera_angle and explicit fire, without mouse/drag');
  if(input.drag&&(input.mouse||keys.length||input.duration_ms<100||Object.keys(input.drag).some(k=>!['from','to'].includes(k))||['from','to'].some(k=>!input.drag[k]||Object.keys(input.drag[k]).some(v=>!['x','y'].includes(v))||!Number.isFinite(input.drag[k].x)||!Number.isFinite(input.drag[k].y)||input.drag[k].x<0||input.drag[k].y<0||input.drag[k].x>=800||input.drag[k].y>=600)))throw Error('Native drag requires only bounded from/to canvas points, no keys/mouse, and at least 100ms');
  return {client,input};
}
async function nativeAct(value) {
  if(value.pauseAfter===false)throw Error('Standalone native actions must pause before returning; capture during a bounded action instead');
  if(Object.keys(value).some(k=>!['op','actor','input','pauseAfter','expect','require','sequence'].includes(k)))throw Error('Unknown native action field');
  if(value.expect&&(!value.expect.world||typeof value.expect.world!=='string'||Object.keys(value.expect).some(k=>!['world','items','inventory_revision'].includes(k))))throw Error('Native expect requires world and optional items/inventory_revision');
  if(value.sequence&&value.input)throw Error('Choose one input or a bounded sequence');
  const sequence=value.sequence??[{actor:value.actor,input:value.input}];
  if(!Array.isArray(sequence)||!sequence.length||sequence.length>32)throw Error('Native sequence requires 1–32 explicit actor/input steps');
  const groups=sequence.map(v=>(Array.isArray(v)?v:[v]).map(v=>{if(!v||Object.keys(v).some(k=>!['actor','input'].includes(k)))throw Error('Native sequence supports only actor/input');return nativeStep(v.actor,v.input);}));
  for(const group of groups)if(!group.length||new Set(group.map(s=>s.client.actor)).size!==group.length)throw Error('Parallel native inputs require distinct configured actors');
  const steps=groups.flat();if(steps.length>32)throw Error('Native sequence exceeds 32 inputs');
  if(steps.reduce((sum,s)=>sum+s.input.duration_ms,0)>10000)throw Error('Native sequence duration exceeds 10 seconds');
  const before=await api({op:'observe',actor:value.actor});
  if(before.controller!=='native'||!before.lifecycle.alive)throw Error('Native input requires the same living native actor');
  if(value.require&&(value.require.world!==before.world||value.require.inventory_revision!==undefined&&value.require.inventory_revision!==before.character.inventory_revision))throw Error('Native action precondition changed');

  await resumeAll();
  if(value.require){const current=await api({op:'observe',actor:value.actor});if(current.world!==value.require.world||value.require.inventory_revision!==undefined&&current.character.inventory_revision!==value.require.inventory_revision)throw Error('Native action precondition changed during resume');}
  let primary;const cleanup=[],stepEvidence=[];
  try{
   await eachClient(clients,client=>client.evaluate('document.querySelector("ruffle-player").focus({preventScroll:true})'));
   for(const group of groups){const prepared=await Promise.allSettled(group.map(async({client,input})=>{
    const current=await api({op:'observe',actor:client.actor});
    if(current.controller!=='native'||!current.lifecycle.alive)throw Error('Native sequence requires living configured participants');
    const keys=input.keys;
    if(keys.includes('Insert')&&current.world==='Nexus')throw Error('Native escape is already in Nexus');
    let mouse,dragTo;
    if(input.mouse||input.drag||input.target){
      const rect=await client.evaluate('(()=>{const r=document.querySelector("ruffle-player").getBoundingClientRect();return{x:r.x,y:r.y,width:r.width,height:r.height,viewportWidth:innerWidth,viewportHeight:innerHeight}})()');
      if(rect.width!==800||rect.height!==600||rect.x<0||rect.y<0||rect.x+800>rect.viewportWidth||rect.y+600>rect.viewportHeight)throw Error('Native client is clipped or scaled');
      let from=input.drag?input.drag.from:input.mouse;
      if(input.target){
        const known=(await api({op:'inspect',actor:client.actor})).view;
        const target=known.objects.find(e=>e.id===input.target.entity&&e.hp>0);
        if(!target)throw Error('Native target is not in the actual client neighborhood');
        const a=input.target.camera_angle,dx=target.position.x-known.position.x,dy=target.position.y-known.position.y,x=50*(dx*Math.cos(a)+dy*Math.sin(a)),y=50*(-dx*Math.sin(a)+dy*Math.cos(a)),scale=Math.max(1,Math.hypot(x,y)/270);
        from={x:300+x/scale,y:325+y/scale};
      }
      mouse={x:rect.x+from.x,y:rect.y+from.y};
      if(input.drag)dragTo={x:rect.x+input.drag.to.x,y:rect.y+input.drag.to.y};
      // Ordered CDP input needs no paint between pointer placement and DOWN.
      await client.call('Input.dispatchMouseEvent',{type:'mouseMoved',...mouse});
    }
    return async()=>{
    const press=await client.hold(keys,input.target?.fire===false?undefined:mouse,input.duration_ms,dragTo);
    for(const result of await press.down)if(result.status==='rejected')throw result.reason;
    await client.evaluate('new Promise(ok=>requestAnimationFrame(()=>requestAnimationFrame(ok)))');
    await states(true);
    const observed=await api({op:'observe',actor:client.actor});
    stepEvidence.push({actor:client.actor,before_ms:current.game_ms,game_ms:observed.game_ms,input_started_wall_ms:press.started,input_release_wall_ms:press.released,position:observed.position,character:observed.character,own_projectiles:input.target?(await api({op:'inspect',actor:client.actor})).diagnostic.own_projectiles:[]});
    };
  }));for(const result of prepared)if(result.status==='rejected')throw result.reason;
  const results=await Promise.allSettled(prepared.map(result=>result.value()));for(const result of results)if(result.status==='rejected')throw result.reason;}}catch(error){primary=error;}
  try{await pauseAll(value.expect);}catch(error){cleanup.push(String(error));}
  if(primary||cleanup.length)throw Error(JSON.stringify({primary:primary?String(primary):null,cleanup,steps:stepEvidence}));
  const after=await api({op:'observe',actor:value.actor});
  if(value.expect?.items)for(const[slot,item]of Object.entries(value.expect.items))if(!/^(?:[0-9]|1[01])$/.test(slot)||after.character.items[Number(slot)]!==item)throw Error('Expected native inventory slot did not commit: '+slot);
  if(value.expect?.inventory_revision!==undefined&&after.character.inventory_revision!==value.expect.inventory_revision)throw Error('Expected native inventory revision did not commit');
  const world_changed=after.session.world!==before.session.world;
  const effects={distance:world_changed?null:Math.hypot(after.position.x-before.position.x,after.position.y-before.position.y),shots:after.verification.shots-before.verification.shots,hp:after.character.hp-before.character.hp,mp:after.character.mp-before.character.mp,inventory_changed:JSON.stringify(after.character.items)!==JSON.stringify(before.character.items),world_changed};
  return {id,steps:stepEvidence,status:value.expect?'outcome_complete':'input_complete',execution:'native',effects,paused:after.paused,pause_scope:'server_and_native',native_clock:'pause_adjusted_page_time',actor:value.actor,state:after};
}
let result;
try {
  if(config.native!==undefined && !Array.isArray(config.native)) throw new Error('native must be an explicit array');
  const actors=new Set(),targets=new Set();
  for(const spec of config.native??[]) {
    const key=`${spec.port}/${spec.target}`;
    if(actors.has(spec.actor)||targets.has(key))throw new Error('Native actors and targets must each be unique');
    actors.add(spec.actor);targets.add(key);clients.push(await connect(spec));
  }
  if(clients.length&&command.pauseAfter===false)throw Error('Native participants require bounded operations that pause before returning');
  const started=performance.now();
  if(command.op==='pause') result=await pauseAll();
  else if(command.op==='resume') {if(clients.length)throw Error('Native resume requires a bounded action with automatic pause');await resumeAll();result={paused:false};}
  else if(command.op==='act'&&(command.input||command.sequence)) result=await nativeAct(command);
  else if(['act','run'].includes(command.op)&&clients.length&&command.pauseAfter!==false) {
    await resumeAll();
    let primary;
    try {result=await api({...command,pauseAfter:false},id);} catch(e){primary=e;}
    let cleanup;
    try {await pauseAll();} catch(e){cleanup=e;}
    if(primary||cleanup) throw new Error(JSON.stringify({primary:primary?String(primary):null,pause:cleanup?String(cleanup):null}));
    const state=await api({op:'observe',actor:command.actor});
    if(result.result?.state)result.result.state=state;else if(result.result?.actor)result.result=state;
    result={...result,paused:true,pause_scope:'server_and_native'};
  } else result=await api(command,id);
  await endLease();
  result={...result,client_wall_ms:Math.round(performance.now()-started)};
  await save(result);
  console.log(JSON.stringify(mutating?{...result,evidence:path}:result));
} catch(error) {
  let pauseError;
  if(mayProgress) try {await pauseAll();await endLease();} catch(e){pauseError=String(e);try{await abortLease();}catch(halt){pauseError+='; native safety halt unconfirmed: '+halt;}}
  const failed={id,status:'failed',error:String(error),pause_error:pauseError};
  try {await save(failed);} catch(e){failed.evidence_error=String(e);}
  console.error(JSON.stringify(failed));
  process.exitCode=1;
} finally {clearInterval(leaseTimer);for(const c of clients)c.close();}
