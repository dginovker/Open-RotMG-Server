import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {mkdir, open, readFile, writeFile, realpath, rename, lstat, access} from 'node:fs/promises';
import {resolve, dirname, join, isAbsolute} from 'node:path';
import {pathToFileURL, fileURLToPath} from 'node:url';
import {randomBytes, randomUUID, createHash} from 'node:crypto';
import {writeSync,readFileSync} from 'node:fs';
import {startChild,stopChild,identity,registry} from './native-test-process.mjs';
import {createServer} from 'node:net';
import {parseArgs} from 'node:util';
import {setTimeout as delay} from 'node:timers/promises';
import assert from 'node:assert/strict';
import {connect} from './record-demo.mjs';

const exec = promisify(execFile), root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
let activeCase=false;
export function browserDiagnostic(line, closing=false) {
  const m=/^\[\d+:\d+:\d+\/\d+\.\d+:ERROR:([^\]]+):\d+\] (.*)$/.exec(line);
  // Push registration is outside this local game's protocol; retain its diagnostics without inventing game failures.
  if(m&&['google_apis/gcm/engine/registration_request.cc','google_apis/gcm/engine/mcs_client.cc'].includes(m[1]))return 'background-push';
  if(m&&m[1]==='dbus/object_proxy.cc'&&m[2]==='Failed to call method: org.freedesktop.DBus.Properties.GetAll: object_path= /org/freedesktop/UPower/devices/DisplayDevice: org.freedesktop.DBus.Error.ServiceUnknown: The name org.freedesktop.UPower was not provided by any .service files')return 'unavailable-power-service';
  if(closing&&m&&m[1]==='gpu/command_buffer/service/shared_image/shared_image_manager.cc'&&m[2]==='SharedImageManager::ProduceSkia: Trying to Produce a Skia representation from a non-existent mailbox.')return 'renderer-shutdown';
  return /\bERROR\b|panicked/.test(line)?'fatal':null;
}
export {startChild,stopChild} from './native-test-process.mjs';
export function hasNativeEntry(module) {
  const present=Object.hasOwn(module,'enter');
  assert(!present||typeof module.enter==='function','Case enter export must be a function when present');
  return present;
}
export async function nativeEntry(module,client,{signal,stopped,check},automatic) {
  check();
  if(!hasNativeEntry(module)){await automatic();check();return;}
  const {actor,account,browser}=client;
  assert(Number.isSafeInteger(actor)&&actor>0&&Number.isSafeInteger(account)&&account>0,'Entry requires actual actor/account IDs');
  const {owner,port,target,pid}=browser;
  assert(typeof owner==='string'&&owner&&typeof target==='string'&&target&&Number.isInteger(port)&&port>0&&port<=65535&&Number.isSafeInteger(pid)&&pid>0,'Entry requires the validated managed browser receipt');
  const context=Object.freeze({actor,account,browser:Object.freeze({owner,port,target,pid}),signal});
  await Promise.race([Promise.resolve().then(()=>{check();return module.enter(context);}),stopped]);
  check();
}
export async function nativeBeforeNavigate(module,browser,{signal,stopped,check}) {
  check();if(!Object.hasOwn(module,'beforeNavigate'))return;
  assert(typeof module.beforeNavigate==='function','beforeNavigate must be a function');
  const {owner,port,target,pid}=browser;
  assert(typeof owner==='string'&&owner&&typeof target==='string'&&target&&Number.isInteger(port)&&port>0&&port<=65535&&Number.isSafeInteger(pid)&&pid>0,'Validated managed browser receipt required');
  const context=Object.freeze({browser:Object.freeze({owner,port,target,pid}),signal});
  await Promise.race([Promise.resolve().then(()=>{check();return module.beforeNavigate(context);}),stopped]);check();
}
export async function prepareFixture(module, api, signal) {
  signal.throwIfAborted();let open=true;
  const access=async command=>{
    signal.throwIfAborted();assert(open,'Fixture preparation phase is closed');
    assert(['capabilities','catalog','world_info','observe','inspect'].includes(command.op)||(command.op==='spawn'&&command.spec?.controller==='direct'&&command.spec.profile===null),'Fixture preparation allows only paused reads and explicit direct scout spawn');
    const result=await api(command);signal.throwIfAborted();assert(open,'Fixture preparation phase is closed');return result;
  };
  let abort;
  const cancelled=new Promise((_,reject)=>{abort=()=>reject(signal.reason);signal.addEventListener('abort',abort,{once:true});});
  try {
    assert(module.prepare===undefined||typeof module.prepare==='function','Case prepare must be a function');
    const spec=await Promise.race([module.prepare===undefined?module.fixture:module.prepare(access),cancelled]);
    assert(spec&&typeof spec==='object'&&['class','level','seed','world','position','equipment','backpack','stats'].every(k=>Object.hasOwn(spec,k)),'Case must return an explicit fixture');
    const items=(values,n)=>Array.isArray(values)&&values.length===n&&values.every(v=>Number.isInteger(v)&&v>=-1&&v<=65535);
    assert(Number.isInteger(spec.class)&&spec.class>=0&&spec.class<=65535&&Number.isInteger(spec.level)&&spec.level>=1&&spec.level<=20&&Number.isSafeInteger(spec.seed)&&spec.seed>=0&&(Number.isInteger(spec.world)||typeof spec.world==='string')&&(spec.position==='spawn'||typeof spec.position==='function'||(spec.position&&Number.isFinite(spec.position.x)&&Number.isFinite(spec.position.y)))&&items(spec.equipment,4)&&items(spec.backpack,8)&&(spec.stats===null||(Array.isArray(spec.stats)&&spec.stats.length===8&&spec.stats.every(v=>Number.isInteger(v)&&v>=0&&v<=2147483647))),'Invalid explicit fixture bounds');
    assert(spec.worldSeed===undefined||typeof spec.world==='string'&&Number.isSafeInteger(spec.worldSeed)&&spec.worldSeed>=0,'worldSeed requires a dungeon template and a nonnegative safe integer');
    return spec;
  } finally {open=false;signal.removeEventListener('abort',abort);}
}
export function nativeJoins({signal,api,participants,freePort,privateDisplay,openClient}) {
  let joining=false;
  const check=()=>signal.throwIfAborted();
  return {
    idle(){assert(!joining,'Native case returned with a pending join');},
    async add(spec){
      check();assert(!joining,'A native client join is already pending');joining=true;
      try {
        spec=await prepareFixture({fixture:spec},api,signal);check();
        assert(participants.length<3,'At most three prepared native clients');
        const world=typeof spec.world==='string'?(await api({op:'world',template:spec.world,...(spec.worldSeed===undefined?{}:{seed:spec.worldSeed})})).result.world:spec.world;
        check();assert(Number.isInteger(world),'Prepared fixture must resolve to an explicit world ID');
        const port=await freePort();check();
        const env=await privateDisplay(participants.length);check();
        return await openClient(spec,world,port,env);
      } finally {joining=false;}
    },
  };
}
export function nativeConnections({signal,closing,cleanupError,diagnose}) {
  const owned=new Set();
  return {
    async open(connect,use){
      signal.throwIfAborted();const connection=await connect();
      if(signal.aborted){
        try{connection.close();}catch(error){
          const failure=new AggregateError([signal.reason,error],'Aborted native join: '+String(signal.reason)+'; connection close failed: '+String(error));
          diagnose(String(failure));if(closing())cleanupError(failure);throw failure;
        }
        signal.throwIfAborted();
      }
      owned.add(connection);
      return await use(connection);
    },
    close(){for(const connection of owned){owned.delete(connection);try{connection.close();}catch(error){cleanupError(error);}}},
  };
}
export async function spawnNativeFixture(api,spec,worldId,guid) {
  assert(Number.isInteger(worldId),'Prepared fixture must resolve to an explicit world ID');
  const world=await api({op:'world_info',world:worldId});const point=spec.position==='spawn'?world.spawn_points[0]:null;
  if(spec.position==='spawn')assert(Array.isArray(point)&&point.length===2&&point.every(Number.isFinite),'Invalid observed spawn tuple');
  const position=spec.position==='spawn'?{x:point[0],y:point[1]}:typeof spec.position==='function'?spec.position(world):spec.position;
  assert(position&&Number.isFinite(position.x)&&Number.isFinite(position.y),'Prepared fixture must resolve to an explicit finite position');
  const {worldSeed,...actorSpec}=spec;
  const created=await api({op:'spawn',spec:{...actorSpec,world:worldId,guid,position,controller:'native',profile:null}});
  return {created,position};
}
export async function runNativeScenario(run,game,idle,report) {
  report.before=await game.observe();await run(game);idle();report.after=await game.observe();idle();
}
export async function nativeXauth(exec,auth,display,cookie,signal) {
  signal.throwIfAborted();return await exec('xauth',['-f',auth,'add',display,'MIT-MAGIC-COOKIE-1',cookie],{signal});
}
async function freePort(requested=0) {
  const server=createServer();await new Promise((ok,bad)=>{server.once('error',bad);server.listen(requested,'127.0.0.1',ok);});
  const port=server.address().port;await new Promise((ok,bad)=>server.close(error=>error?bad(error):ok()));return port;
}
export async function closedArchive(options) {
  const {ownerRun,sourceHead}=options;assert(isAbsolute(ownerRun)&&resolve(ownerRun)===ownerRun);
  assert(/^[a-f0-9]{40}$/.test(sourceHead)&&await realpath(ownerRun)===ownerRun,'Explicit source/canonical owner root required');
  const current=(await exec('git',['rev-parse','HEAD'],{cwd:root})).stdout.trim(),dirty=(await exec('git',['status','--porcelain'],{cwd:root})).stdout.trim();
  assert(current===sourceHead&&!dirty,'Archive requires current clean actual source');
  const previous=JSON.parse(await readFile(join(ownerRun,'result.json'),'utf8')),held=JSON.parse(await readFile(join(ownerRun,'children.json'),'utf8'));
  assert(held.boot===await readFile('/proc/sys/kernel/random/boot_id','utf8')&&previous.source.head===sourceHead&&!previous.source.dirty&&held.done&&previous.status!=='running'&&Array.isArray(previous.cleanupErrors)&&previous.cleanupErrors.length===0,'Owned prior lifecycle must be terminal and clean');
  assert(held.children.length>0&&held.children.every(c=>previous.children.some(x=>x.pid===c.pid)),'Every held creation must have its actual close receipt');
  assert(previous.participants.length===1&&previous.participants[0].actor===previous.actor&&previous.participants[0].account===previous.account&&Number.isSafeInteger(previous.actor)&&previous.actor>0&&Number.isSafeInteger(previous.account)&&previous.account>0,'Actual positive actor/account required');
  const known=new Set([...held.children.map(c=>c.pid),previous.guardian]);assert(known.size===held.children.length+1,'Duplicate cached PID/guardian identity');
  assert(previous.children.every(c=>known.has(c.pid))&&previous.children.some(c=>c.name==='guardian'&&c.pid===previous.guardian),'Complete held cleanup receipts required');
  for(const pid of known){assert(Number.isSafeInteger(pid)&&pid>0);try{await access('/proc/'+pid);}catch(error){if(error.code==='ENOENT')continue;throw error;}throw Error('Known prior PID is not absent; no adoption or reuse');}
  const origin=new URL(previous.origin),port=Number(origin.port);
  assert(origin.protocol==='http:'&&origin.hostname==='127.0.0.1'&&!origin.username&&!origin.password&&origin.pathname==='/'&&!origin.search&&!origin.hash&&port>0&&port<=65535&&previous.origin==='http://127.0.0.1:'+port,'Exact prior loopback origin required');
  assert(Number.isSafeInteger(held.deadline)&&held.deadline-Date.now()>120000,'Original absolute campaign cleanup reserve exhausted');
  const state=join(ownerRun,'state'),profile=join(ownerRun,'profile-0');
  for(const p of [ownerRun,state,profile]){assert(await realpath(p)===p);const s=await lstat(p);assert(s.isDirectory()&&s.uid===process.getuid()&&(s.mode&0o077)===0,'Private owned directory required');}
  for(const name of ['SingletonLock','SingletonCookie','SingletonSocket']){
    try{await lstat(join(profile,name));}catch(error){if(error.code==='ENOENT')continue;throw error;}
    throw Error('Prior managed profile is not closed');
  }
  return Object.freeze({state,profile,actor:previous.actor,account:previous.account,sourceHead,ownerRun,port,deadline:held.deadline});
}
export async function runCase(scenario, options) {
  const archive=Object.hasOwn(options,'archive')?await closedArchive(options.archive):null;
  assert(options.runDir && isAbsolute(options.runDir)&&resolve(options.runDir)===options.runDir,'A canonical absolute new --run-dir is required');
  assert(options.runDir!==root&&!options.runDir.startsWith(root+'/'),'Keep generated runs outside the source checkout');
  assert(options.browserTools && isAbsolute(options.browserTools),'Set PI_BROWSER_TOOLS to the installed managed-browser directory');
  assert(options.chrome && isAbsolute(options.chrome),'Set PI_NATIVE_TEST_CHROME to the Chrome executable');
  assert(Array.isArray(options.chromeArgs) && options.chromeArgs.every(x=>typeof x==='string'),'Chrome arguments must be an explicit JSON array');
  if(options.client!==undefined)assert(isAbsolute(options.client),'--client must be an absolute checkout path');
  const timeout=options.timeoutMs;assert(Number.isInteger(timeout)&&timeout>=180000&&timeout<=3600000,'Timeout must be 180000–3600000ms, including 120 seconds for cleanup');
  assert(!activeCase,'Only one native case may own this process');
  const out=options.runDir,state=archive?archive.state:join(out,'state');assert.equal(await realpath(dirname(out)),dirname(out),'Run parent must be canonical, not a symlink');await mkdir(out,{mode:0o700});activeCase=true;
  const previousOwner=process.env.PI_BROWSER_OWNER;
  const controller=new AbortController(), children=[], files=[], participants=[], ownedBrowsers=[], report={status:'running',scenario,prepared:true,phases:{},assertions:[],captures:[],children:[]};
  const started=performance.now();let phase='setup',connection,owner,port,chromePort,browser,actor,account,primary,closing=false,guardian,registryState;
  const elapsed=()=>Math.round(performance.now()-started);
  let failureClient;
  let saveTurn=Promise.resolve();
  const save=async()=>{
    const bytes=JSON.stringify({...report,wallMs:elapsed()},null,2)+'\n',previous=saveTurn;
    let release;saveTurn=new Promise(ok=>{release=ok;});
    await previous;
    // Aborted bounded work can still be publishing when failure/cleanup requests the next save.
    try{await writeFile(join(out,'result.tmp'),bytes,{mode:0o600});await rename(join(out,'result.tmp'),join(out,'result.json'));}finally{release();}
  };
  const register=()=>{registryState.children=children.filter(r=>r.identity).map(r=>({...r.identity,name:r.name}));registry(join(out,'children.json'),registryState);};
  const cleanupCauses=[],cleanupError=error=>{cleanupCauses.push(error);report.cleanupErrors.push(String(error));};
  const fail=error=>{if(closing)cleanupError(error);else if(!controller.signal.aborted)controller.abort(error);};
  const interrupted=()=>fail(Error('Native test interrupted'));
  process.once('SIGINT',interrupted);process.once('SIGTERM',interrupted);
  const timer=setTimeout(()=>fail(Error('Native test active deadline reached; cleanup reserve retained')),(archive?archive.deadline-Date.now():timeout)-120000);
  let rejectStop;const stopped=new Promise((_,bad)=>{rejectStop=bad;});stopped.catch(()=>{});
  controller.signal.addEventListener('abort',()=>rejectStop(controller.signal.reason),{once:true});
  const check=()=>controller.signal.throwIfAborted();
  const connections=nativeConnections({signal:controller.signal,closing:()=>closing,cleanupError,diagnose:message=>console.error(message)});
  const bounded=async(name,work)=>{check();phase=name;report.phase=name;await save();check();const t=performance.now();try{return await Promise.race([work(),stopped]);}finally{report.phases[name]=(report.phases[name]??0)+Math.round(performance.now()-t);}};
  const log=async name=>{const f=await open(join(out,name),'a',0o600);files.push(f);return f.fd;};
  const launch=(name,command,args,stdio,env=process.env)=>{
    check();const row=startChild(command,args,{cwd:root,stdio,env},controller.signal);row.name=name;children.push(row);
    row.started.then(register).catch(fail);
    row.closed.then(value=>{row.done=true;if(!row.stopping)fail(Error(name+' exited before cleanup: '+JSON.stringify(value)));});register();return row;
  };
  const waitFor=async(work,description,limit=30000)=>{
    const end=performance.now()+limit;
    while(true){check();const value=await work();if(value)return value;assert(performance.now()<end,description);await delay(30,undefined,{signal:controller.signal});}
  };
  const api=async command=>{
    check();const token=(await readFile(join(state,'control.token'),'utf8')).trim();
    const response=await fetch(`http://127.0.0.1:${port}/dev/testing`,{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify({id:'case-'+randomUUID(),command}),signal:AbortSignal.any([controller.signal,AbortSignal.timeout(10000)])});
    const value=await response.json();assert(response.ok&&value.status!=='failed','Test API failed: '+JSON.stringify(value));return value;
  };
  const call=(method,params={})=>{check();return connection.call(method,params);};
  const evaluate=expression=>{check();return connection.eval(expression);};
  async function driver(command) {
    check();const id='case-'+randomUUID(),request=join(out,id+'.request.json');await writeFile(request,JSON.stringify({id,command}),{mode:0o600});check();
    const row=launch('controller',process.execPath,[join(root,'scripts/game.mjs'),join(out,'native.json'),request],['ignore','pipe','pipe']);
    const fd=await log('controller.stderr');let text='',errors='';row.child.stdout.on('data',b=>text+=b);
    row.child.stderr.on('data',b=>{writeSync(fd,b);errors+=b;let n;while((n=errors.indexOf('\n'))>=0){const line=errors.slice(0,n);errors=errors.slice(n+1);if(line.startsWith('{'))try{const value=JSON.parse(line);if(value.id===id&&value.status==='failed')fail(Error('Controller failed: '+value.error));}catch(error){fail(error);}}});
    row.stopping=true;const deadline=setTimeout(()=>fail(Error('Controller command exceeded 90 seconds')),90000);let status;
    try{await row.started;status=await Promise.race([row.closed,stopped]);}finally{clearTimeout(deadline);}
    await writeFile(join(out,id+'.result.json'),text,{mode:0o600});assert.equal(status.code,0,'Controller failed; see controller.stderr');check();return JSON.parse(text);
  }
  async function capture(name,source=connection) {
    assert(/^[A-Za-z0-9_-]+$/.test(name),'Capture name must be a filename');
    const image=await source.call('Page.captureScreenshot',{format:'png'});const path=join(out,name+'.png');await writeFile(path,Buffer.from(image.data,'base64'),{mode:0o600});report.captures.push(path);return path;
  }
  try {
    await save();
    registryState={parent:identity(process.pid),boot:readFileSync('/proc/sys/kernel/random/boot_id','utf8'),deadline:archive?archive.deadline:Date.now()+timeout,children:[],done:false};register();
    guardian=startChild(process.execPath,[join(root,'scripts/native-test-process.mjs'),out],{stdio:['ignore','ignore',await log('guardian.log')]},controller.signal);
    guardian.closed.then(value=>{if(!closing)fail(Error('Cleanup guardian exited early: '+JSON.stringify(value)));});
    report.guardian=guardian.child.pid;await guardian.started;
    report.inputs={};for(const [name,path] of [['case',resolve(scenario)],['runner',fileURLToPath(import.meta.url)],['controller',join(root,'scripts/game.mjs')]]){
      const bytes=await readFile(path);report.inputs[name]=createHash('sha256').update(bytes).digest('hex');
      if(name==='case')await writeFile(join(out,'case.mjs'),bytes,{mode:0o600});
    }
    const fixtureModule=await import(pathToFileURL(resolve(scenario)).href);if(archive)assert(typeof fixtureModule.readArchive==='function'&&!Object.hasOwn(fixtureModule,'prepare')&&!Object.hasOwn(fixtureModule,'enter')&&!Object.hasOwn(fixtureModule,'fixture')&&!Object.hasOwn(fixtureModule,'run'),'Archive requires readArchive only: no fixture/prepare/enter/run');else assert(typeof fixtureModule.run==='function'&&(fixtureModule.fixture||fixtureModule.prepare!==undefined),'Case must export fixture or prepare(api), and run(game)');hasNativeEntry(fixtureModule);
    let spec=archive?null:fixtureModule.prepare===undefined?await prepareFixture(fixtureModule,api,controller.signal):null;
    port=await freePort(archive?archive.port:0);chromePort=await freePort();assert.notEqual(port,chromePort,'Ports collided; no launch attempted');
    owner='native-test:'+randomUUID();process.env.PI_BROWSER_OWNER=owner;
    report.source={head:(await exec('git',['rev-parse','HEAD'],{cwd:root})).stdout.trim(),dirty:Boolean((await exec('git',['status','--porcelain'],{cwd:root})).stdout.trim())};report.origin=`http://127.0.0.1:${port}`;
    await bounded('prepareBuildStart',async()=>{
      const began=performance.now();let buildStart,buildEnd,ready;
      const args=[String(port),archive?'--state':'--test-state',state,'--no-shore-bots','--release'];if(archive)args.push('--dev-control');if(options.client)args.push('--client',options.client);
      const server=launch('server',join(root,'run.sh'),args,['ignore','pipe','pipe'],{...process.env,CARGO_TERM_COLOR:'never'});
      for(const [stream,name] of [[server.child.stdout,'server.log'],[server.child.stderr,'build.log']]){
        const fd=await log(name);let pending='';
        stream.on('data',bytes=>{
          try{
            writeSync(fd,bytes);pending+=bytes;
            if(pending.includes('Building and starting your local game.'))buildStart??=performance.now();
            if(pending.includes('Finished `release` profile'))buildEnd??=performance.now();
            let newline;while((newline=pending.indexOf('\n'))>=0){const line=pending.slice(0,newline);pending=pending.slice(newline+1);if(line.startsWith('{')){const event=JSON.parse(line);if(event.level==='ERROR')fail(Error('Backend ERROR: '+line));if(event.fields?.message==='game frontdoor listening')ready=performance.now();}}
          }catch(error){fail(error);}
        });
        stream.on('end',()=>{if(pending.includes('"level":"ERROR"'))fail(Error('Backend ERROR at EOF: '+pending));});
      }
      await server.started;await waitFor(()=>ready,'Server did not become ready',timeout-125000);
      assert(buildStart&&buildEnd&&buildStart<=buildEnd&&buildEnd<=ready,'Missing or inconsistent launcher/Cargo timing boundaries');
      report.phases.clientPreparation=Math.round(buildStart-began);report.phases.build=Math.round(buildEnd-buildStart);report.phases.serverStart=Math.round(ready-buildEnd);
      report.client=options.client?{mode:'local',path:options.client,lock:JSON.parse(await readFile(join(options.client,'ruffle/lock.json'),'utf8'))}:{mode:'deployed',selection:JSON.parse(await readFile(join(state,'client-selection.json'),'utf8'))};
    });
    delete report.phases.prepareBuildStart;
    if(!archive&&fixtureModule.prepare!==undefined)spec=await bounded('fixturePreparation',()=>prepareFixture(fixtureModule,api,AbortSignal.any([controller.signal,AbortSignal.timeout(30000)])));
    report.fixture=spec;
    async function privateDisplay(index){
      check();const auth=join(out,'Xauthority-'+index);await writeFile(auth,'',{mode:0o600,signal:controller.signal});const cookie=randomBytes(16).toString('hex');
      check();
      await nativeXauth(exec,auth,':99',cookie,controller.signal);
      const xvfb=launch('Xvfb','Xvfb',['-displayfd','3','-screen','0','1400x1000x24','-nolisten','tcp','-auth',auth],['ignore','ignore',await log('display.log'),'pipe']);
      let display='';xvfb.child.stdio[3].on('data',b=>display+=b);await waitFor(()=>display.includes('\n'),'Private display did not start',10000);display=':'+display.trim();
      check();await nativeXauth(exec,auth,display,cookie,controller.signal);
      const env={...process.env,DISPLAY:display,XAUTHORITY:auth,PI_BROWSER_OWNER:owner,PI_BROWSER_TAB_POLICY:'agent_only',CHROME_REAL_BINARY:options.chrome};delete env.WAYLAND_DISPLAY;
      const box=launch('openbox','openbox',['--sm-disable'],['ignore',await log('window-manager.log'),await log('window-manager.log')],env);await box.started;
      return env;
    }
    const joins=nativeJoins({signal:controller.signal,api,participants,freePort,privateDisplay,openClient});
    const addClient=spec=>joins.add(spec);
    async function openClient(spec,worldId,chromePort,env){
      let connection,browser,actor,account;
      const call=(method,params={})=>{check();return connection.call(method,params);};
      const evaluate=expression=>{check();return connection.eval(expression);};
      const chrome=launch('Chrome',process.execPath,[join(options.browserTools,'launch-browser.mjs'),`--remote-debugging-port=${chromePort}`,`--user-data-dir=${archive?archive.profile:join(out,'profile-'+participants.length)}`,'--ozone-platform=x11','--window-size=1280,960','--window-position=0,0','--no-first-run','--no-default-browser-check',...options.chromeArgs,'about:blank'],['ignore','pipe','pipe'],env);
      const browserLog=await log('browser.log');let browserLines='';
      const inspectLine=line=>{const kind=browserDiagnostic(line,closing);if(kind==='fatal')fail(Error('Browser ERROR: '+line));else if(kind)(report.browserDiagnostics??=[]).push({kind,line});};
      chrome.child.stderr.on('data',bytes=>{try{writeSync(browserLog,bytes);browserLines+=bytes;let n;while((n=browserLines.indexOf('\n'))>=0){inspectLine(browserLines.slice(0,n));browserLines=browserLines.slice(n+1);}}catch(error){fail(error);}});
      chrome.child.stderr.on('end',()=>{if(browserLines)inspectLine(browserLines);});
      let receipt='';chrome.child.stdout.on('data',b=>receipt+=b);await waitFor(()=>receipt.includes('\n'),'Managed browser did not return its receipt',30000);browser=JSON.parse(receipt.split('\n')[0]);assert(browser.target&&browser.owner===owner&&browser.pid===chrome.child.pid,'Invalid managed browser receipt');
      ownedBrowsers.push({...browser,port:chromePort});report.browser={owner,port:chromePort,target:browser.target,pid:browser.pid};
      if(archive){actor=archive.actor;account=archive.account;}
      let resolveGuest,rejectGuest;const guest=new Promise((ok,bad)=>{resolveGuest=ok;rejectGuest=bad;});guest.catch(()=>{});let requests=0,guestDone=false;
      return await connections.open(()=>connect({port:chromePort,target:browser.target,url:'about:blank',helper:join(options.browserTools,'direct-cdp.mjs'),onEvent:event=>{
        if(event.method==='Runtime.exceptionThrown'){fail(Error('Browser exception: '+JSON.stringify(event.params.exceptionDetails)));return;}
        if(event.method==='Runtime.consoleAPICalled'&&event.params.type==='error'){fail(Error('Browser console error: '+JSON.stringify(event.params.args)));return;}
        if(event.method!=='Fetch.requestPaused')return;
        (async()=>{
          assert.equal(++requests,1,'Only one normal fresh guest per client is permitted');const e=event.params;
          assert.equal(e.request.url,report.origin+'/legacy-api/char/list');assert.equal(e.request.method,'POST');
          const form=new URLSearchParams(e.request.postData),guid=form.get('guid');assert(guid&&/^[A-Za-z0-9_.-]{1,128}$/.test(guid)&&!form.get('password'),'Expected a UI-generated disposable guest');
          const {created,position}=await spawnNativeFixture(api,spec,worldId,guid);actor=created.result.actor;account=created.result.account;
          await writeFile(join(out,participants.length?'fixture-'+participants.length+'.json':'fixture.json'),JSON.stringify({actor,account,spec:{...spec,world:worldId,position},prepared:true}),{mode:0o600});
          await call('Fetch.continueRequest',{requestId:e.requestId});guestDone=true;resolveGuest();
        })().catch(error=>{rejectGuest(error);fail(error);});
      }}),async opened=>{
      connection=opened;
      const window=await call('Browser.getWindowForTarget',{targetId:browser.target}),bounds=(await call('Browser.getWindowBounds',{windowId:window.windowId})).bounds;
      assert(bounds.left>=0&&bounds.left+bounds.width<=1400&&bounds.width<=1300,'Owned native windows must remain unoccluded on the private display');
      // Guest setup can fail before the first participant is published.
      if(participants.length===0)failureClient={connection,browser,chromePort};
      report.bootstrapStep='configure-page';
      await call('Runtime.enable');
      if(!archive)await call('Fetch.enable',{patterns:[{urlPattern:report.origin+'/legacy-api/char/list',requestStage:'Request'}]});
      report.bootstrapStep='before-navigate-hook';
      await nativeBeforeNavigate(fixtureModule,{owner:browser.owner,port:chromePort,target:browser.target,pid:browser.pid},{signal:controller.signal,stopped,check});
      report.bootstrapStep='navigate';await call('Page.navigate',{url:report.origin});
      if(!archive){report.bootstrapStep='fresh-guest-char-list';await Promise.race([guest,waitFor(()=>guestDone,'Normal fresh guest char/list missing',60000),stopped]);report.bootstrapStep='disable-guest-interception';await call('Fetch.disable');}

      report.bootstrapStep='wait-ruffle-playing';
      await waitFor(()=>evaluate('document.querySelector("ruffle-player")?.ruffle().isPlaying===true'),'Ruffle did not start');
      if(archive){report.bootstrapStep='read-archive';await Promise.race([Promise.resolve().then(()=>{check();return fixtureModule.readArchive(Object.freeze({actor,account,origin:report.origin,browser:Object.freeze({owner,port:chromePort,target:browser.target,pid:browser.pid}),signal:controller.signal}));}),stopped]);check();report.bootstrapStep='ready';return {actor,account,connection,browser,chromePort};}
      report.bootstrapStep='native-entry';
      await nativeEntry(fixtureModule,{actor,account,browser:{owner:browser.owner,port:chromePort,target:browser.target,pid:browser.pid}},{signal:controller.signal,stopped,check},async()=>{
      report.bootstrapStep='close-hardware-modal';
      await evaluate('(()=>{const p=document.querySelector("ruffle-player");const modal=p.shadowRoot.querySelector("#hardware-acceleration-modal");if(modal&&!modal.classList.contains("hidden"))modal.querySelector(".close-modal").click();})()');
      const click=async()=>{
        report.bootstrapStep='measure-native-menu';
        const r=await evaluate('(()=>{const r=document.querySelector("ruffle-player").getBoundingClientRect();return{x:r.x,y:r.y,width:r.width,height:r.height,vw:innerWidth,vh:innerHeight};})()');assert(r.width===800&&r.height===600&&r.x>=0&&r.y>=0&&r.x+800<=r.vw&&r.y+600<=r.vh,'Native menu requires the unclipped 800x600 client');
        const p={x:r.x+400,y:r.y+550,button:'left',clickCount:1};
        report.bootstrapStep='click-native-menu';
        try{await call('Input.dispatchMouseEvent',{type:'mousePressed',buttons:1,...p});}finally{await connection.call('Input.dispatchMouseEvent',{type:'mouseReleased',buttons:0,...p});}
        report.bootstrapStep='wait-native-menu-frames';
        await evaluate('new Promise(ok=>requestAnimationFrame(()=>requestAnimationFrame(ok)))');
      };
      report.bootstrapStep='focus-native-menu';
      await call('Emulation.setFocusEmulationEnabled',{enabled:true});await click();await click();
      });
      report.bootstrapStep='wait-character-load';
      await waitFor(async()=>{const actors=[...participants.map(c=>c.actor),actor],s=await api({op:'native_state',actors});assert(!s.stopped,'Native session stopped');assert(!s.actors.some(a=>['dead','dying','lost'].includes(a.stage)),'Native participant terminal');return s.actors.length===actors.length&&s.actors.every(a=>a.stage==='ready');},'Normal character LOAD did not complete');
      report.bootstrapStep='publish-participant';
      participants.push({actor,account,connection,browser,chromePort,call,evaluate,bounds,display:env.DISPLAY});
      await writeFile(join(out,'native.json'),JSON.stringify({origin:report.origin,state,browserTools:join(options.browserTools,'direct-cdp.mjs'),native:participants.map(c=>({port:c.chromePort,target:c.browser.target,actor:c.actor}))}),{mode:0o600});
      report.bootstrapStep='pause-native-client';
      await driver({op:'pause'});report.participants=participants.map(c=>({actor:c.actor,account:c.account,browser:c.browser,bounds:c.bounds,display:c.display}));
      report.bootstrapStep='ready';await save();check();
      return participants.at(-1);
      });
    }
    const first=await bounded('browserAndFixture',()=>archive?privateDisplay(0).then(env=>openClient(null,null,chromePort,env)):addClient(spec));
    ({actor,account,connection,browser,chromePort}=first);report.actor=actor;report.account=account;
    if(!archive)await bounded('scenario',async()=>{
      const gameFor=client=>{const {actor,evaluate}=client;return {actor,join:async spec=>gameFor(await addClient(spec)),observe:()=>api({op:'observe',actor}),inspect:()=>api({op:'inspect',actor}),check(name,condition){assert(condition,name);report.assertions.push(name);},
        async act(input,captureSpec){
          check();let complete=false;const action=driver({op:'act',actor,...(Array.isArray(input)?{sequence:input}:{input}),pauseAfter:true}).finally(()=>complete=true);
          if(!captureSpec)return await action;
          const shot=(async()=>{
            await waitFor(async()=>{assert(!complete,'Action ended before screenshot condition');const view=await api({op:'observe',actor});return !view.paused&&captureSpec.when(view)&&await evaluate('document.querySelector("ruffle-player").ruffle().isPlaying');},'Screenshot condition not observed',(Array.isArray(input)?input.flat().reduce((sum,s)=>sum+s.input.duration_ms,0):input.duration_ms)+10000);
            assert(await evaluate('document.querySelector("ruffle-player").ruffle().isPlaying'),'Client paused before capture');await capture(captureSpec.name,client.connection);
          })();
          const results=await Promise.allSettled([action,shot]);for(const r of results)if(r.status==='rejected')throw r.reason;return results[0].value;
        }};};
      const game=gameFor(first);
      await runNativeScenario(game=>fixtureModule.run(game),game,()=>joins.idle(),report);report.activeGameMs=report.after.game_ms-report.before.game_ms;
    });
    check();joins.idle();report.status='passed';
  } catch(error) {
    primary=error;report.status='failed';report.failure={phase,error:String(error),stack:error.stack};
    if(failureClient)report.failure.captureBrowser={...failureClient.browser,port:failureClient.chromePort};
    await save();
    if(failureClient)try{const image=await failureClient.connection.call('Page.captureScreenshot',{format:'png'});await writeFile(join(out,'failure.png'),Buffer.from(image.data,'base64'),{mode:0o600});report.failure.capture=join(out,'failure.png');}catch(error){report.failure.captureError=String(error);}
  } finally {
    clearTimeout(timer);closing=true;report.cleanupErrors=[];controller.abort(Error('Native test closing'));const cleanup=performance.now();
    for(const row of children)row.stopping=true;
    let managed,tabs=[];
    if(ownedBrowsers.length)try{
      managed=await import(pathToFileURL(join(options.browserTools,'tabs.mjs')).href);
      for(const b of ownedBrowsers)for(const page of (await managed.liveTargets(b.port)).targets.filter(p=>p.type==='page')){
        const tab={...b,target:page.targetId};await managed.ensureTarget(tab);managed.assertOwner(tab,owner);tabs.push(tab);
      }
    }catch(error){cleanupError(error);}
    connections.close();
    // One graceful browser shutdown drains renderer work before retiring its owned targets.
    for(const row of children.reverse()){
      row.stopping=true;
      try{report.children.push({name:row.name,...await stopChild(row)});if(row.name==='Chrome')for(const tab of tabs.filter(t=>t.pid===row.child.pid))managed.released(tab);}catch(error){report.children.push({name:row.name,pid:row.child.pid,identity:row.identity,error:String(error)});cleanupError(error);}
    }
    try{if(registryState){registryState.done=report.cleanupErrors.length===0;register();}}catch(error){cleanupError(error);}
    if(guardian)try{
      const closed=await Promise.race([guardian.closed.then(value=>({value})),delay(5000,undefined,{ref:false}).then(()=>null)]);
      if(!closed)cleanupError(Error('Cleanup guardian exceeded original 5000ms wait: '+guardian.child.pid));
      const value=closed?closed.value:await stopChild(guardian,0);report.children.push({name:'guardian',...value});await guardian.started;assert.equal(value.code,0,'Cleanup guardian failed');
    }catch(error){report.children.push({name:'guardian',pid:guardian.child.pid,identity:guardian.identity,error:String(error)});cleanupError(error);}
    if(registryState&&report.cleanupErrors.length)try{registryState.done=false;register();}catch(error){cleanupError(error);}
    for(const f of files)try{await f.close();}catch(error){cleanupError(error);}
    report.phases.cleanup=Math.round(performance.now()-cleanup);if(report.cleanupErrors.length)report.status='failed';
    try{await save();}finally{process.removeListener('SIGINT',interrupted);process.removeListener('SIGTERM',interrupted);activeCase=false;if(previousOwner===undefined)delete process.env.PI_BROWSER_OWNER;else process.env.PI_BROWSER_OWNER=previousOwner;}
  }
  if(primary)throw new AggregateError([primary,...cleanupCauses],'Native test failed during '+report.failure.phase+': '+String(primary)+'; '+join(out,'result.json'),{cause:primary});
  if(report.cleanupErrors.length)throw new AggregateError(cleanupCauses,'Native test cleanup failed: '+report.cleanupErrors.join('; ')+'; '+join(out,'result.json'));
  return {...report,wallMs:elapsed(),report:join(out,'result.json')};
}
if(process.argv[1]&&resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  if(process.argv.length===3&&process.argv[2]==='--help'){
    console.log('Usage: node scripts/native-test.mjs CASE.mjs --run-dir NEW_ABSOLUTE_DIRECTORY\nLinux: Node22.15+, Xvfb, xauth, Openbox and managed browser helpers.\nSet PI_BROWSER_TOOLS and PI_NATIVE_TEST_CHROME, or --browser-tools and --chrome.\nOptional --chrome-args JSON, --client ABSOLUTE_CHECKOUT, --timeout-ms (default900000; includes120000 cleanup reserve).\nFresh prepared fixtures only. Existing directories are refused; state/logs are preserved.');
  }else try {
    const {values,positionals}=parseArgs({allowPositionals:true,options:{'run-dir':{type:'string'},'browser-tools':{type:'string'},chrome:{type:'string'},'chrome-args':{type:'string'},client:{type:'string'},'timeout-ms':{type:'string',default:'900000'}}});
    assert.equal(positionals.length,1,'Usage: native-test.mjs CASE.mjs --run-dir NEW_ABSOLUTE_DIRECTORY');
    const result=await runCase(positionals[0],{runDir:values['run-dir'],browserTools:values['browser-tools']??process.env.PI_BROWSER_TOOLS,chrome:values.chrome??process.env.PI_NATIVE_TEST_CHROME,chromeArgs:JSON.parse(values['chrome-args']??process.env.PI_NATIVE_TEST_CHROME_ARGS??'[]'),client:values.client,timeoutMs:Number(values['timeout-ms'])});
    console.log(JSON.stringify({status:result.status,assertions:result.assertions,phases:result.phases,wallMs:result.wallMs,browserDiagnostics:result.browserDiagnostics,report:result.report}));
  }catch(error){console.error(String(error));process.exitCode=1;}
}
