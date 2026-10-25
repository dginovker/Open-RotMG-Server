import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
import {parseArgs} from 'node:util';
import {setTimeout as delay} from 'node:timers/promises';
import {createHash} from 'node:crypto';

const here = path.dirname(fileURLToPath(import.meta.url));
const pause = (ms, signal) => delay(ms, undefined, {signal});
function requireValue(ok, message) { if (!ok) throw Error(message); }
export function media(args) {
  const result = spawnSync('python3', [path.join(here, 'demo_media.py'), ...args], {encoding:'utf8'});
  if (result.error) throw result.error;
  requireValue(result.status === 0, `Media validation failed: ${result.stderr || result.stdout}`);
  return result.stdout;
}
export async function socket(url, Socket = WebSocket, onEvent) {
  const ws = new Socket(url), pending = new Map(); let sequence = 0;
  const fail = message => { for (const p of pending.values()) { clearTimeout(p.timer); p.reject(Error(message)); } pending.clear(); };
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.close(); reject(Error('CDP connection deadline')); }, 10000);
    ws.onopen = () => { clearTimeout(timer); resolve(); };
    ws.onerror = () => { clearTimeout(timer); reject(Error('CDP connection failed')); };
  });
  ws.onclose = () => fail('CDP socket closed');
  ws.onerror = () => fail('CDP socket error');
  ws.onmessage = event => {
    let message;
    try { message = JSON.parse(event.data); } catch { fail('Malformed CDP response'); ws.close(); return; }
    if (message.method) { onEvent?.(message); return; }
    const p = pending.get(message.id);
    if (!p) return;
    pending.delete(message.id); clearTimeout(p.timer);
    message.error ? p.reject(Error(JSON.stringify(message.error))) : p.resolve(message.result);
  };
  return {
    call(method, params = {}) {
      requireValue(ws.readyState === 1, 'CDP socket is not open');
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => { pending.delete(id); reject(Error(`CDP deadline: ${method}`)); }, 10000);
        pending.set(id, {resolve, reject, timer});
        try { ws.send(JSON.stringify({id, method, params})); }
        catch (error) { pending.delete(id); clearTimeout(timer); reject(error); }
      });
    },
    close() { fail('CDP connection released'); ws.close(); },
  };
}
export async function connect({port, target, url, helper, onEvent}) {
  requireValue(Number.isInteger(port) && port >= 1024 && port <= 65535, 'Explicit local Chrome port required');
  requireValue(helper && path.isAbsolute(helper), 'Explicit absolute managed-CDP helper path required');
  const helpers = await import(pathToFileURL(helper).href);
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`, {signal:AbortSignal.timeout(10000)})).json();
  const matches = pages.filter(p => p.type === 'page' && p.id === target && p.url === url);
  requireValue(matches.length === 1, 'Fresh owned target/URL missing or ambiguous');
  const raw = await socket(matches[0].webSocketDebuggerUrl, WebSocket, onEvent);
  try {
    const call = await helpers.managedCDP(port, target, helpers.scriptOwner(), raw.call);
    return {call, close:raw.close, async eval(expression) {
      const r = await call('Runtime.evaluate', {expression, returnByValue:true, awaitPromise:true});
      requireValue(!r.exceptionDetails, `Page evaluation failed: ${JSON.stringify(r.exceptionDetails)}`);
      return r.result.value;
    }};
  } catch (error) { raw.close(); throw error; }
}

export function browser(command, value) {
  const find = root => [...root.querySelectorAll('*')].flatMap(e => [
    ...(e.tagName === 'CANVAS' ? [e] : []), ...(e.shadowRoot ? find(e.shadowRoot) : []),
  ]);
  const canvases = find(document);
  if (canvases.length !== 1) throw Error('Expected one original game canvas');
  const canvas = canvases[0];
  const geometry = () => {
    if (document.hidden) throw Error('Owned canvas page is hidden');
    const b = canvas.getBoundingClientRect();
    if (!canvas.width || !canvas.height || b.width <= 0 || b.height <= 0 || b.x < 0 || b.y < 0 || b.right > innerWidth || b.bottom > innerHeight) throw Error('Canvas is empty or clipped outside the input viewport');
    return {x:b.x, y:b.y, width:b.width, height:b.height, pixels:[canvas.width,canvas.height]};
  };
  let r = window.__rotmgDemo;
  if (command === 'geometry' || command === 'gesture') {
    const b = geometry();
    if (command === 'gesture' && r) {
      if (r.active !== null) throw Error('Another input gesture is active');
      r.rect = b; r.active = r.gestures.length;
      r.gestures.push({id:r.active, start:r.now()-r.start, end:null});
    }
    return b;
  }
  if (command === 'endGesture') { if (r) { r.gestures[r.active].end = r.now()-r.start; r.active = null; } return; }
  if (command === 'start') {
    if (r) throw Error('Recording already active; refusing another controller');
    // Encoded video timestamps include pauses that the native playback clock excludes.
    const native=globalThis[Symbol.for('openrotmg.testing.native-clock.v4')];
    if(native&&value!=='native_wall')throw Error('Paused native clocks require explicit native_wall recording');
    const now=value==='native_wall'?native?.wallNow:performance.now.bind(performance);
    if(typeof now!=='function')throw Error('Native wall recording requires the current clock; reopen the same living character');
    const rect = geometry(), mime = 'video/webm;codecs=vp8';
    if (!MediaRecorder.isTypeSupported(mime)) throw Error('VP8 MediaRecorder is unavailable');
    const stream = canvas.captureStream(60), recorder = new MediaRecorder(stream, {mimeType:mime, videoBitsPerSecond:2000000});
    r = window.__rotmgDemo = {recorder, stream, rect, now, size:rect.pixels, chunks:[], inputs:[], gestures:[], active:null, failure:null, rafCount:0};
    r.update = () => {
      try { r.rect = geometry(); if (canvas.width !== r.size[0] || canvas.height !== r.size[1]) r.failure = 'Canvas dimensions changed during recording'; }
      catch (error) { r.failure = error.message; }
    };
    r.resize = new ResizeObserver(r.update); r.resize.observe(canvas);
    window.addEventListener('scroll', r.update, true);
    document.addEventListener('visibilitychange', r.update);
    r.input = e => {
      if (document.activeElement?.type === 'password') { r.failure = 'Password input during capture is forbidden'; return; }
      const event = {type:e.type, time:now()-r.start};
      if (e.type.startsWith('mouse')) {
        event.x = (e.clientX-r.rect.x)*r.size[0]/r.rect.width;
        event.y = (e.clientY-r.rect.y)*r.size[1]/r.rect.height;
        event.inside = event.x >= 0 && event.y >= 0 && event.x < r.size[0] && event.y < r.size[1];
        event.buttons = e.buttons; event.gesture = r.active;
        window.__rotmgDemoPointer = {event, time:r.start+event.time};
      } else {
        // Printable text can contain credentials; only timing, not its contents, belongs in the sidecar.
        event.key = e.key.length > 1 ? e.key : e.code === 'Space' ? 'Space' : 'text';
      }
      r.inputs.push(event);
    };
    r.events = ['mousemove','mousedown','mouseup','keydown','keyup'];
    recorder.ondataavailable = e => { if (e.data.size) r.chunks.push(e.data); };
    recorder.onerror = e => { r.failure = e.error.message; };
    r.start = now(); r.mediaStartOffset = null;
    const last = window.__rotmgDemoPointer;
    if (last) r.inputs.push({...last.event,time:last.time-r.start,gesture:null});
    recorder.onstart = () => { r.mediaStartOffset = now()-r.start; };
    for (const name of r.events) window.addEventListener(name, r.input, {capture:true, passive:true});
    const frame = () => { r.rafCount++; r.raf = requestAnimationFrame(frame); }; frame();
    // Waiting for onstart can deadlock a canvas that first repaints in response to input.
    recorder.start(1000);
    return {size:r.size, clock:'performance.now at recorder.start'};
  }
  if (!r) throw Error('No recording is present');
  if (command === 'stop') return new Promise((resolve, reject) => {
    r.recorder.onstop = () => {
      r.duration = (r.now()-r.start)/1000;
      for (const name of r.events) window.removeEventListener(name, r.input, true);
      r.resize.disconnect(); window.removeEventListener('scroll', r.update, true); document.removeEventListener('visibilitychange', r.update); cancelAnimationFrame(r.raf);
      r.stream.getTracks().forEach(t => t.stop());
      const blob = new Blob(r.chunks, {type:'video/webm'}), reader = new FileReader();
      reader.onerror = () => reject(Error('Recorded Blob read failed'));
      reader.onload = () => {
        r.base64 = reader.result.split(',')[1];
        resolve({version:1, size:r.size, duration:r.duration, bytes:blob.size, length:r.base64.length, inputs:r.inputs, gestures:r.gestures, failure:r.failure, rafCount:r.rafCount, clock:'performance.now at recorder.start', mediaStartOffset:r.mediaStartOffset});
      };
      reader.readAsDataURL(blob);
    };
    r.recorder.stop();
  });
  if (command === 'chunk') return r.base64.slice(value, value+200000);
  if (command === 'clear') { delete window.__rotmgDemo; return; }
  throw Error('Unknown recorder command');
}
const page = (c, command, value = null) => c.eval(`(${browser.toString()})(${JSON.stringify(command)},${JSON.stringify(value)})`);
export function point(rect, xy) {
  requireValue(xy.length === 2 && xy.every(n => Number.isFinite(n) && n >= 0 && n < 1), 'Coordinates must be normalized canvas points in [0,1)');
  return {x:rect.x+xy[0]*rect.width, y:rect.y+xy[1]*rect.height};
}
export function input(c, {now = () => performance.now(), wait = pause} = {}) {
  const keys = new Map(), abort = new AbortController(); let closed = false;
  const waiting = ms => {
    requireValue(Number.isFinite(ms) && ms >= 0 && ms <= 120000 && !closed, 'Wait must be bounded and the input controller open');
    return wait(ms, abort.signal);
  };
  const send = (method, params) => { requireValue(!closed, 'Input controller is closed'); return c.call(method, params); };
  const moved = (rect, xy) => send('Input.dispatchMouseEvent', {type:'mouseMoved', ...point(rect,xy)});
  return {
    wait:waiting,
    async move(xy) { await moved(await page(c, 'geometry'), xy); },
    async gesture(from, to, milliseconds = 800, hz = 60) {
      requireValue(Number.isFinite(milliseconds) && milliseconds > 0 && milliseconds <= 10000 && Number.isFinite(hz) && hz >= 30 && hz <= 120, 'Gesture must be bounded, at 30–120 requested Hz');
      requireValue(from.some((n,i) => n !== to[i]), 'Stationary holds belong in wait(), not gesture()');
      const rect = await page(c, 'gesture'); point(rect,from); point(rect,to);
      const begin = now();
      try {
        await moved(rect, from);
        while (now()-begin < milliseconds) {
          const next = (Math.floor((now()-begin)*hz/1000)+1)*1000/hz;
          await waiting(Math.max(0, begin+next-now()));
          const fraction = Math.min(1, (now()-begin)/milliseconds);
          await moved(rect, from.map((n,i) => n+(to[i]-n)*fraction));
        }
      } finally { await page(c, 'endGesture'); }
    },
    async click(xy) {
      const rect = await page(c, 'geometry'), p = point(rect,xy);
      await moved(rect, xy);
      await send('Input.dispatchMouseEvent', {type:'mousePressed', ...p, button:'left', buttons:1, clickCount:1});
      try { await waiting(60); } finally { await c.call('Input.dispatchMouseEvent', {type:'mouseReleased', ...p, button:'left', buttons:0, clickCount:1}); }
    },
    async key(key, milliseconds = 120) {
      requireValue(Number.isFinite(milliseconds) && milliseconds >= 0 && milliseconds <= 5000, 'Key hold must be bounded');
      const named = {Insert:['Insert',45], Escape:['Escape',27], Enter:['Enter',13], Space:['Space',32], ArrowLeft:['ArrowLeft',37], ArrowUp:['ArrowUp',38], ArrowRight:['ArrowRight',39], ArrowDown:['ArrowDown',40]};
      requireValue(key in named || /^[a-z0-9]$/i.test(key), 'Unsupported native key');
      const [code, virtual] = key in named ? named[key] : [/^[0-9]$/.test(key) ? `Digit${key}` : `Key${key.toUpperCase()}`, key.toUpperCase().charCodeAt(0)];
      const p = {key:key === 'Space' ? ' ' : key, code, windowsVirtualKeyCode:virtual};
      requireValue(!keys.has(code), 'Native key is already held');
      keys.set(code,p); await send('Input.dispatchKeyEvent', {type:'keyDown', ...p});
      try { await waiting(milliseconds); } finally { await c.call('Input.dispatchKeyEvent', {type:'keyUp', ...p}); keys.delete(code); }
    },
    async release() { closed = true; abort.abort(); for (const [code,p] of keys) { await c.call('Input.dispatchKeyEvent', {type:'keyUp', ...p}); keys.delete(code); } },
  };
}
export async function transfer(c, directory, name) {
  const meta = await page(c, 'stop');
  await fs.writeFile(path.join(directory, `${name}.json`), JSON.stringify(meta,null,2), {flag:'wx'});
  requireValue(meta.length > 0 && meta.bytes > 0, 'Empty recorded Blob; inspect paint/occlusion');
  const handle = await fs.open(path.join(directory, `${name}.webm.partial`), 'wx'); let bytes = 0;
  try {
    for (let offset=0; offset<meta.length; offset+=200000) {
      const part = await page(c, 'chunk', offset);
      requireValue(typeof part === 'string' && part.length === Math.min(200000,meta.length-offset) && /^[A-Za-z0-9+/]*={0,2}$/.test(part), `Truncated or invalid Blob chunk at ${offset}`);
      const data = Buffer.from(part,'base64'); await handle.writeFile(data); bytes += data.length;
    }
  } finally { await handle.close(); }
  requireValue(bytes === meta.bytes, 'Recorded byte count mismatch');
  await fs.rename(path.join(directory, `${name}.webm.partial`), path.join(directory, `${name}.webm`));
  await page(c,'clear'); requireValue(meta.failure === null, `Recorder failure: ${meta.failure}`);
  return meta;
}
export async function visible(c, target, {now = () => performance.now(), wait = pause} = {}) {
  const deadline=now()+5000; let observed;
  do {
    const {bounds}=await c.call('Browser.getWindowForTarget',{targetId:target});
    observed={windowState:bounds.windowState, hidden:await c.eval('document.hidden')};
    if (['normal','maximized','fullscreen'].includes(observed.windowState) && observed.hidden===false) return observed;
    await wait(100);
  } while(now()<deadline);
  throw Error('Owned window visibility deadline; restore the owned window before recording: '+JSON.stringify(observed));
}
export async function capture(options) {
  const {out, scenario, seconds} = options;
  requireValue(Number.isFinite(seconds) && seconds >= 16 && seconds <= 120, 'Explicit capture duration must be 16–120 seconds');
  const directory = path.resolve(out), root = path.resolve(here,'..');
  requireValue(directory !== root && !directory.startsWith(root+path.sep), 'Keep generated media outside the source repository');
  const driver = await import(pathToFileURL(path.resolve(scenario)).href);
  requireValue(typeof driver.run === 'function', 'Scenario module must export run(input)');
  await fs.mkdir(path.dirname(directory), {recursive:true}); await fs.mkdir(directory);
  const c = await connect(options), controls = input(c); let active = null, error = null, timer, busy = false;
  const receipt = {version:1, status:'recording', scenarioSha256:createHash('sha256').update(await fs.readFile(scenario)).digest('hex'), scope:'Recording quality only; scenario return values are not independently verified gameplay outcomes.'};
  let rejectInterrupt;
  const interrupted = new Promise((_,reject) => { rejectInterrupt=reject; });
  interrupted.catch(()=>{});
  const interrupt = signal => {
    receipt.interrupted=signal; rejectInterrupt(Error(`Recording interrupted: ${signal}`));
    controls.release().catch(failure => { (receipt.cleanupErrors ??= []).push(failure.message); });
  };
  const sigint=()=>interrupt('SIGINT'), sigterm=()=>interrupt('SIGTERM');
  process.once('SIGINT',sigint); process.once('SIGTERM',sigterm);
  try {
    busy = await c.eval('Boolean(window.__rotmgDemo)');
    requireValue(!busy, 'Another recorder is active; refuse overlapping controllers');
    receipt.window=await visible(c,options.target,{wait:controls.wait});
    await controls.wait(2000);
    await page(c,'start'); active='probe'; await controls.wait(150);
    await controls.gesture([.35,.4],[.65,.6],1500); await controls.wait(300);
    const probe = await transfer(c,directory,'probe'); active=null;
    media(['check','--source',path.join(directory,'probe.webm'),'--meta',path.join(directory,'probe.json'),'--start','0','--seconds',String(probe.duration),'--report',path.join(directory,'probe-quality.json')]);
    await page(c,'start'); active='source'; const begin=performance.now();
    const deadline = new Promise((_,reject) => { timer=setTimeout(() => reject(Error('Scenario exceeded the capture deadline')),seconds*1000); });
    receipt.scenarioResult = await Promise.race([driver.run(controls),deadline,interrupted]); clearTimeout(timer);
    await controls.wait(Math.max(0,seconds*1000-(performance.now()-begin)));
    await transfer(c,directory,'source'); active=null; receipt.status='recorded';
  } catch (failure) { error=failure; busy ||= failure.message.includes('Recording already active'); receipt.status='failed'; receipt.error=failure.message; }
  finally {
    process.removeListener('SIGINT',sigint); process.removeListener('SIGTERM',sigterm);
    clearTimeout(timer);
    for (const cleanup of [() => controls.release(), async () => { if (active) await transfer(c,directory,active); }, async () => { if (!busy) await c.call('Page.close'); }]) {
      try { await cleanup(); } catch (failure) { (receipt.cleanupErrors ??= []).push(failure.message); error ??= failure; }
    }
    c.close(); await fs.writeFile(path.join(directory,'capture.json'),JSON.stringify(receipt,null,2));
  }
  if (error) throw error;
  return receipt;
}
export async function attachment(local, url) {
  requireValue(/^https:\/\/github\.com\/user-attachments\/assets\/[a-f0-9-]+$/.test(url), 'Explicit GitHub attachment URL required');
  const response = await fetch(url, {signal:AbortSignal.timeout(30000)});
  requireValue(response.ok && response.headers.get('content-type')?.startsWith('video/mp4'), 'Attachment is not a downloadable MP4');
  const original=await fs.readFile(local), remote=Buffer.from(await response.arrayBuffer());
  requireValue(original.equals(remote), 'Uploaded attachment bytes differ from the local export');
  return {asset:url, bytes:remote.length, sha256:createHash('sha256').update(remote).digest('hex'), byteEqual:true};
}
export async function player(c, asset) {
  return c.eval(`(async()=>{
    const asset=${JSON.stringify(asset)}, id=asset.split('/').pop();
    const deadline=performance.now()+7000;let matches=[];
    while(performance.now()<deadline){
      matches=[...document.querySelectorAll('video')].filter(v=>[v.src,...[...v.querySelectorAll('source')].map(s=>s.src)].some(src=>{if(!src)return false;const u=new URL(src,location.href);return src===asset || (u.hostname==='private-user-images.githubusercontent.com' && u.pathname.endsWith('-'+id+'.mp4'));}));
      if(matches.length===1 && matches[0].readyState>=2)break;
      await new Promise(r=>setTimeout(r,100));
    }
    if(matches.length!==1 || matches[0].readyState<2)throw Error('Expected unique ready inline attachment player is missing');
    const v=matches[0];
    v.muted=true;await v.play();await new Promise(r=>setTimeout(r,1000));const first=v.currentTime;await new Promise(r=>setTimeout(r,600));
    const result={duration:v.duration,width:v.videoWidth,height:v.videoHeight,controls:v.controls,readyState:v.readyState,error:v.error?.message??null,timeAdvanced:v.currentTime>first};v.pause();
    if(result.error || !result.timeAdvanced || result.readyState<3 || result.duration!==15 || result.width!==1920 || result.height!==1080 || !result.controls)throw Error('Inline video playback failed: '+JSON.stringify(result));
    return result;
  })()`);
}
function gh(args) { const r=spawnSync('gh',args,{encoding:'utf8'}); if(r.error)throw r.error;requireValue(r.status===0,r.stderr);return r.stdout; }
export async function publish({take, repo, pr, port, target, helper}) {
  requireValue(/^[\w.-]+\/[\w.-]+$/.test(repo) && /^[1-9]\d*$/.test(String(pr)), 'Explicit repository and PR number required');
  const directory=path.resolve(take), local=path.join(directory,'demo.mp4');
  requireValue(!(await fs.readdir(directory)).includes('publication.json'), 'Publication receipt already exists; refuse duplicate upload');
  media(['verify','--video',local]);
  requireValue(gh(['pr','comment','--help']).includes('--attach'), 'Installed gh lacks --attach');
  requireValue(gh(['api',`repos/${repo}`,'--jq','.permissions.push']).trim()==='true', 'No push access to the explicit upload destination');
  const url=gh(['pr','view',String(pr),'--repo',repo,'--json','url','--jq','.url']).trim();
  const c=await connect({port,target,url,helper});
  try {
    const comment=gh(['pr','comment',String(pr),'--repo',repo,'--attach',local]).trim();
    const comments=JSON.parse(gh(['pr','view',String(pr),'--repo',repo,'--json','comments'])).comments;
    const body=comments.find(v=>v.url===comment)?.body, assets=body?.match(/https:\/\/github\.com\/user-attachments\/assets\/[a-f0-9-]+/g);
    requireValue(assets?.length===1,'Published comment has no unique video attachment');
    const checked=await attachment(local,assets[0]);
    await c.call('Page.navigate',{url}); await pause(1500);
    checked.playback=await player(c,assets[0]); checked.comment=comment;
    await fs.writeFile(path.join(directory,'publication.json'),JSON.stringify(checked,null,2),{flag:'wx'});
    return checked;
  } finally { try { await c.call('Page.close'); } finally { c.close(); } }
}
if (process.argv[1] && import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href) {
  const [command,...args]=process.argv.slice(2);
  try {
    if(command==='record' || command==='publish') {
      const names=command==='record'?['port','target','url','out','scenario','helper','seconds']:['take','repo','pr','port','target','helper'];
      const {values}=parseArgs({args,options:Object.fromEntries(names.map(n=>[n,{type:'string'}]))});
      for(const name of names)requireValue(values[name],`Missing --${name}`);
      values.port=Number(values.port);if(command==='record')values.seconds=Number(values.seconds);
      console.log(JSON.stringify(await (command==='record'?capture(values):publish(values)),null,2));
    } else if(command==='render' || command==='check' || command==='verify') console.log(media([command,...args]));
    else throw Error('Usage: node scripts/record-demo.mjs record|render|check|verify|publish [explicit options]');
  } catch(error) { console.error(error.stack);process.exitCode=1; }
}
