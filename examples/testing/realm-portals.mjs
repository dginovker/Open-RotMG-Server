import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';

export const realmSeed=12351;
let api;
const spec={class:0x30e,level:20,seed:901,world:-2,position:'spawn',equipment:[0xa97,0xa2e,0xb05,0xac5],backpack:Array(8).fill(-1),stats:[670,385,50,25,50,50,40,75]};
export async function prepare(){
  const root=join(process.env.QA_RESULTS_DIR,'native'),{origin}=JSON.parse(await readFile(join(root,'result.json'),'utf8'));
  const token=(await readFile(join(root,'state','control.token'),'utf8')).trim();
  api=async command=>{const r=await fetch(origin+'/dev/testing',{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify({id:crypto.randomUUID(),command}),signal:AbortSignal.timeout(10000)});const value=await r.json();assert(r.ok&&value.status==='succeeded',JSON.stringify(value));return value;};
  const response=await fetch(origin+'/dev/testing',{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify({id:'portal-capacity',command:{op:'prepare_realm_capacity',limit:1}}),signal:AbortSignal.timeout(10000)});
  const receipt=await response.json();assert(response.ok&&receipt.status==='succeeded',JSON.stringify(receipt));
  await writeFile(join(process.env.QA_RESULTS_DIR,'capacity.json'),JSON.stringify(receipt));
  return spec;
}
export async function run(game){
  const observations=[];
  async function save(name,client=game){
    const view=await client.observe();
    view.entrances=(await client.inspect()).view.objects.filter(o=>o.kind===0x712);
    observations.push({name,view});
    await writeFile(join(process.env.QA_RESULTS_DIR,'portals.json'),JSON.stringify(observations,null,2));return view;
  }
  async function capture(name){
    await game.act({keys:[],duration_ms:350});
    await game.act({keys:[],duration_ms:700},{name,when:()=>true});
  }
  async function walk(pos){
    for(let i=0;i<80;i++){
      const v=await game.observe(),dx=pos.x-v.position.x,dy=pos.y-v.position.y,distance=Math.hypot(dx,dy);
      if(distance<.6)return;
      const x=(dx-dy)/Math.SQRT2,y=(dx+dy)/Math.SQRT2,keys=[];
      if(Math.abs(x)>.12)keys.push(x>0?'d':'a');
      if(Math.abs(y)>.12)keys.push(y>0?'s':'w');
      await game.act({keys,duration_ms:Math.min(300,Math.max(50,Math.round(distance*80)))});
    }
    throw Error('Native walk did not reach Realm entrance');
  }
  async function escape(client){
    await client.act({keys:['Insert'],duration_ms:700});
    for(let i=0;i<8&&(await client.observe()).world!=='Nexus';i++)await client.act({keys:[],duration_ms:500});
    assert.equal((await client.observe()).world,'Nexus');
  }
  async function enter(){
    await game.act({keys:[],mouse:{x:700,y:575},duration_ms:700});
    for(let i=0;i<8&&(await game.observe()).world==='Nexus';i++)await game.act({keys:[],duration_ms:500});
    const view=await game.observe();assert.equal(view.world,'Realm');return view;
  }
  const initial=await save('initial');
  let portals=[];
  for(let i=0;i<60&&portals.length<1;i++){
    await game.act({keys:['d','w'],duration_ms:300});
    portals=(await game.inspect()).diagnostic.entities.filter(e=>e.kind===0x712);
  }
  assert.equal(portals.length,1,'Quiet startup must have one Realm entrance');
  await capture('realm-quiet-startup');
  game.check('quiet startup has one maintained entrance',portals.length===1);
  const limited=(await game.inspect()).view.objects.find(o=>o.kind===0x712&&o.name.endsWith('(0/1)'));
  assert(limited,'Prepared capacity entrance missing');
  portals=[portals.find(p=>p.id===limited.id)];
  await walk(portals[0].pos);
  await capture('realm-portals-open');
  const open=await save('open');
  const entrances=open.entrances;
  game.check('initial entrance is available',entrances.length===1&&entrances[0].usable);
  const occupant=await game.join({...spec,seed:902,world:-3});
  const filled=await save('occupant',occupant);
  await api({op:'cheat',actor:occupant.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});
  for(let n=0;n<4;n++)await game.act({keys:[],duration_ms:10000});
  portals=(await game.inspect()).diagnostic.entities.filter(e=>e.kind===0x712);
  assert.equal(portals.length,2,'Sustained full occupancy must grow one independent Realm');
  portals.sort((a,b)=>Number(b.id===limited.id)-Number(a.id===limited.id));
  await walk({x:(portals[0].pos.x+portals[1].pos.x)/2,y:(portals[0].pos.y+portals[1].pos.y)/2});
  game.check('demand growth adds a differently named available entrance',new Set((await game.inspect()).view.objects.filter(o=>o.kind===0x712).map(o=>o.name.split(' (')[0])).size===2);
  await walk(portals[0].pos);
  await capture('realm-portals-full');
  const full=await save('full');
  const unavailable=full.entrances.find(o=>o.id===portals[0].id),available=full.entrances.find(o=>o.id===portals[1].id);
  game.check('live population binds Full only to its occupied Realm',unavailable?.name.endsWith('(1/1)')&&!unavailable.usable&&available?.name.endsWith('(0/85)')&&available.usable);
  await game.act({keys:[],mouse:{x:700,y:575},duration_ms:700});
  const denied=await save('denied');
  game.check('full entrance click retains the exact living Nexus session and inventory',denied.lifecycle.alive&&JSON.stringify(denied.session)===JSON.stringify(full.session)&&JSON.stringify(denied.character.items)===JSON.stringify(initial.character.items));
  await escape(occupant);
  await capture('realm-portals-reopened');
  const reopened=await save('reopened'),entrance=reopened.entrances.find(o=>o.id===portals[0].id);
  game.check('departure restores the same empty available entrance',entrance?.name.endsWith('(0/1)')&&entrance.usable);
  const first=await enter();await save('entered-first');
  game.check('reopened entrance reaches its exact Realm',first.session.world===filled.session.world);
  await escape(game);
  await walk(portals[1].pos);
  const second=await enter();await save('entered-second');
  game.check('second named entrance reaches a different Realm',second.session.world!==first.session.world);
  for(let n=0;n<31;n++){if(n%5===0)await api({op:'cheat',actor:game.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});await game.act({keys:[],duration_ms:10000});}
  game.check('living resident pins the extra beyond five minutes',(await game.observe()).session.world===second.session.world&&(await game.observe()).lifecycle.alive);
  await escape(game);
  await walk({x:(portals[0].pos.x+portals[1].pos.x)/2,y:(portals[0].pos.y+portals[1].pos.y)/2});
  game.check('recently entered extra remains during empty dwell',(await game.inspect()).diagnostic.entities.some(e=>e.id===portals[1].id));
  for(let n=0;n<31;n++)await game.act({keys:[],duration_ms:10000});
  const retired=await save('retired');
  await capture('realm-retired-extra');
  game.check('empty dependency-free extra retires after five simulation minutes',!retired.entrances.some(o=>o.id===portals[1].id)&&retired.entrances.some(o=>o.id===portals[0].id));
  const final=await save('final'),other=await save('other-final',occupant);
  game.check('both accounts remain alive and distinct',final.lifecycle.alive&&other.lifecycle.alive&&final.session.account!==other.session.account);
  game.check('portal round trips preserve character identity, equipment, XP and inventory revision',final.session.character===initial.session.character&&final.session.account===initial.session.account&&JSON.stringify(final.character.items)===JSON.stringify(initial.character.items)&&final.character.experience===initial.character.experience&&final.character.inventory_revision===initial.character.inventory_revision);
}
