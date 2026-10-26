import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {test} from 'node:test';
import vm from 'node:vm';
const source=readFileSync(new URL('./game.mjs',import.meta.url),'utf8');
const body=source.slice(source.indexOf('function verifiedNativeAck('),source.indexOf('async function bind(state)'));
const emit=vm.runInNewContext('('+body+')',{Buffer,Error,Number});
const hash='a'.repeat(64),context={runtime:'actual-native-state-runtime',actor:7,player:9},server={actor:7,player:9,nonce:-1,frame_sha256:hash};
test('verifiedNativeAck: real negative server nonce and actual observed frame are retained distinctly',()=>{let row;emit(context,server,[hash],undefined,b=>{row=JSON.parse(b);return b.length;});assert.deepEqual(row.context,context);assert.deepEqual(row.server,server);assert.equal(row.observedBrowserFrameSha256,hash);assert.equal(row.event,'native_sync_verified_ack');assert(!Object.hasOwn(row.server,'runtime'));});
test('verifiedNativeAck: zero/positive/i32MIN/fraction/unsafe/noninteger nonce never logs success',()=>{for(const nonce of [0,1,-2147483648,-2147483649,-1.5,Number.NaN,'-1',Number.MIN_SAFE_INTEGER]){let writes=0;assert.throws(()=>emit(context,{...server,nonce},[hash],undefined,()=>++writes));assert.equal(writes,0);}});
test('verifiedNativeAck: mismatch and abort never emit a success diagnostic',()=>{for(const [s,frames,abort]of [[{...server,player:10},[hash]], [{...server,actor:8},[hash]],[server,[]],[server,[hash],Error('aborted')]]){let writes=0;assert.throws(()=>emit(context,s,frames,abort,()=>++writes));assert.equal(writes,0);}});
test('verifiedNativeAck: short and thrown logging writes are fatal with original cause',()=>{assert.throws(()=>emit(context,server,[hash],undefined,()=>0),e=>e.cause.message==='Short native_sync ACK diagnostic write');const cause=Error('ENOSPC evidence');assert.throws(()=>emit(context,server,[hash],undefined,()=>{throw cause;}),e=>e.cause===cause);});
test('verifiedNativeAck: diagnostic is after existing exact match and successful browser binding',()=>{const match=source.indexOf("if(actual.player!==acknowledgment.player||!c.frames.includes(acknowledgment.frame_sha256))throw"),binding=source.indexOf("await c.evaluate('globalThis[Symbol.for(\"openrotmg.testing.binding.v1\")]="),diagnostic=source.indexOf('      verifiedNativeAck({runtime:state.runtime');assert(match>0&&binding>match&&diagnostic>binding);assert(!body.includes('api(')&&!body.includes('setTimeout')&&!body.includes('evaluate('));});

test('native resume proves clocks, focus and binding without waiting for extra paints',async()=>{
  const calls=[],before={},after={},client={clock:async value=>calls.push(['clock',value]),call:async method=>calls.push(method),evaluate:async()=>assert.fail('Resume must not wait for a paint')};
  const context={clients:[client],mayProgress:false,leaseError:null,
    states:async(wait,expect,previous)=>{calls.push(wait?'ready':'before');if(wait)assert.equal(previous,before);return wait?after:before;},
    bind:async state=>calls.push(state===before?'bind-before':'bind-after'),initializeClock:async()=>calls.push('initialize'),
    startLease:async()=>calls.push('lease'),api:async command=>calls.push(command.op),eachClient:async(clients,action)=>Promise.all(clients.map(action))};
  const resume=vm.runInNewContext('('+source.slice(source.indexOf('async function resumeAll()'),source.indexOf('const bindings ='))+')',context);
  await resume();
  assert.deepEqual(calls,['before','bind-before','initialize','lease','resume',['clock',false],'Emulation.setFocusEmulationEnabled','ready','bind-after']);
  assert.equal(context.mayProgress,true);
  context.leaseError=Error('renewal failed');await assert.rejects(resume(),/renewal failed/);
});
test('ordinary autofire key uses the same bounded native key validation',()=>{
  const c={clients:[{actor:1}],Error},{step,key}=vm.runInNewContext(source.slice(source.indexOf('const bindings ='),source.indexOf('async function nativeAct('))+';({step:nativeStep,key:bindings.i})',c);
  assert.deepEqual(Array.from(key),['KeyI',73]);
  assert.deepEqual(Array.from(step(1,{keys:['i'],duration_ms:150}).input.keys),['i']);
  assert.throws(()=>step(1,{keys:['i','i'],duration_ms:150}),/unique/);
  assert.throws(()=>step(1,{keys:['i'],duration_ms:0}),/duration_ms/);
});

test('native input retains its post-release paint and readiness proof, not a pre-DOWN paint',()=>{
  const moved=source.indexOf("await client.call('Input.dispatchMouseEvent',{type:'mouseMoved',...mouse})"),hold=source.indexOf('const press=await client.hold',moved),release=source.indexOf('for(const result of await press.down)',hold),paint=source.indexOf('requestAnimationFrame',release),ready=source.indexOf('await states(true)',paint);
  assert(moved>0&&hold>moved&&release>hold&&paint>release&&ready>paint);
  assert(!source.slice(moved,hold).includes('requestAnimationFrame'));
});
