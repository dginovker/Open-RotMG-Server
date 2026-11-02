import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {randomUUID} from 'node:crypto';

export const realmSeed=12351;
const spec={class:0x30e,level:20,seed:901,world:-2,position:'spawn',equipment:[0xa97,0xa2e,0xb05,0xac5],backpack:Array(8).fill(-1),stats:[670,385,50,25,50,50,40,60]};
let api,prepared;
export async function prepare(){
  const root=join(process.env.QA_RESULTS_DIR,'native'),{origin}=JSON.parse(await readFile(join(root,'result.json'),'utf8'));
  const token=(await readFile(join(root,'state','control.token'),'utf8')).trim();
  api=async(command,success=true)=>{
    const r=await fetch(origin+'/dev/testing',{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify({id:randomUUID(),command}),signal:AbortSignal.timeout(10000)});
    const receipt=await r.json();assert.equal(r.ok&&receipt.status==='succeeded',success,JSON.stringify(receipt));return receipt;
  };
  await api({op:'prepare_realm_capacity',limit:1});
  prepared=(await api({op:'prepare_realm_closure',world:-3})).result;
  await writeFile(join(process.env.QA_RESULTS_DIR,'closure-preparation.json'),JSON.stringify(prepared));
  return spec;
}
export async function run(game){
  const samples=[];
  async function save(name,client=game){const state=await client.inspect();samples.push({name,state});await writeFile(join(process.env.QA_RESULTS_DIR,'replacement-journey.json'),JSON.stringify(samples));return state;}
  async function walk(client,pos){
    for(let n=0;n<100;n++){
      const v=await client.observe(),dx=pos.x-v.position.x,dy=pos.y-v.position.y,d=Math.hypot(dx,dy);
      if(d<.6)return;
      const x=(dx-dy)/Math.SQRT2,y=(dx+dy)/Math.SQRT2,keys=[];
      if(Math.abs(x)>.12)keys.push(x>0?'d':'a');if(Math.abs(y)>.12)keys.push(y>0?'s':'w');
      await client.act({keys,duration_ms:Math.min(300,Math.max(50,Math.round(d*80)))});
    }
    throw Error('Native walk did not reach entrance');
  }
  async function hall(client){
    for(let n=0;n<65;n++){
      const state=await client.inspect(),portals=state.diagnostic.entities.filter(e=>e.kind===0x712);
      if(portals.length>=1)return portals;
      await client.act({keys:['d','w'],duration_ms:300});
    }
    throw Error('Two entrances not revealed in Nexus');
  }
  async function enter(client){
    await client.act({keys:[],mouse:{x:700,y:575},duration_ms:1000});
    for(let n=0;n<10&&(await client.observe()).world==='Nexus';n++)await client.act({keys:[],duration_ms:500});
    assert.equal((await client.observe()).world,'Realm');
  }
  async function escape(client){
    await client.act({keys:['Insert'],duration_ms:1000});
    for(let n=0;n<10&&(await client.observe()).world!=='Nexus';n++)await client.act({keys:[],duration_ms:500});
    assert.equal((await client.observe()).world,'Nexus');
  }
  const initial=await save('initial');
  const watcher=await game.join({...spec,seed:902,world:-3});
  await api({op:'cheat',actor:watcher.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});
  for(let n=0;n<4;n++)await game.act({keys:[],duration_ms:10000});
  await escape(watcher);
  let portals=await hall(game);
  for(let n=0;n<65&&portals.length<2;n++){await game.act({keys:['d','w'],duration_ms:300});portals=(await game.inspect()).diagnostic.entities.filter(e=>e.kind===0x712);}
  assert.equal(portals.length,2,'Demand must independently create the second lineage');
  const visible=(await game.inspect()).view.objects;
  // The lowered disposable capacity identifies the exact prepared Realm without changing name policy.
  const old=portals.find(p=>visible.find(o=>o.id===p.id)?.name.endsWith('(0/1)'));
  assert(old,'Selected prepared Realm entrance missing');
  const other=portals.find(p=>p.id!==old.id);
  await walk(game,{x:(old.pos.x+other.pos.x)/2,y:(old.pos.y+other.pos.y)/2});
  await game.act({keys:[],duration_ms:1000},{name:'random-entrances',when:()=>true});
  await walk(game,old.pos);await enter(game);
  assert.equal((await game.observe()).session.world,-3);
  await escape(game);
  game.check('same living character enters and returns through the original entrance', (await game.observe()).session.character===initial.view.session.character);
  await hall(watcher);await walk(watcher,other.pos);
  const unaffected=await save('other-before',watcher);
  const rejected=await api({op:'prepare_realm_closure',world:-3},false);
  game.check('prepared closure cannot be repeated after enrollment',JSON.stringify(rejected).includes('already prepared'));
  await hall(game);await walk(game,old.pos);await enter(game);
  await api({op:'cheat',actor:game.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});
  const kills=(await game.inspect()).diagnostic.character.kills;
  for(let n=0;n<30;n++){
    const s=await game.inspect();if(!s.diagnostic.entities.some(e=>e.id===prepared.hero))break;
    await game.act({keys:[],duration_ms:350,target:{entity:prepared.hero,camera_angle:-Math.PI/4,fire:true}});
  }
  const defeated=await save('final-prepared-hero-defeated');
  game.check('native shot defeats the prepared final hero',defeated.diagnostic.character.kills>kills&&!defeated.diagnostic.entities.some(e=>e.id===prepared.hero));
  const closed=await save('closed-entrance',watcher);
  game.check('only the selected closed Realm entrance disappears',!closed.view.objects.some(o=>o.id===old.id)&&closed.view.objects.some(o=>o.id===other.id));
  for(let n=0;n<16&&(await game.observe()).world==='Realm';n++){
    if(n===6||n===12)await api({op:'cheat',actor:game.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});
    await game.act({keys:[],duration_ms:10000});
  }
  const castle=await save('collective-castle-transfer');
  game.check('real countdown collectively transfers the same living character to Castle',castle.view.map.name==="Oryx's Castle"&&castle.view.session.character===initial.view.session.character&&castle.view.lifecycle.alive);
  await game.act({keys:[],duration_ms:2500});
  await escape(game);await hall(game);
  const replaced=await save('replacement-nexus');
  const replacement=replaced.diagnostic.entities.find(e=>e.kind===0x712&&e.id!==other.id);
  assert(replacement&&replacement.id!==old.id,'Fresh replacement entrance missing');
  const still=replaced.diagnostic.entities.find(e=>e.id===other.id);
  game.check('other entrance stays stationary while replacement receives a fresh legal binding',still&&Math.hypot(still.pos.x-other.pos.x,still.pos.y-other.pos.y)===0&&Math.hypot(replacement.pos.x-other.pos.x,replacement.pos.y-other.pos.y)>=4);
  await walk(game,replacement.pos);
  await game.act({keys:[],duration_ms:1000},{name:'replacement-entrance',when:()=>true});
  await enter(game);const fresh=await save('entered-replacement');
  game.check('replacement entrance reaches a new Realm with the same living identity',fresh.view.session.world!==-3&&fresh.view.session.character===initial.view.session.character&&fresh.view.lifecycle.alive);
  await escape(game);await hall(game);await walk(game,other.pos);await enter(game);
  const otherRealm=await save('entered-unaffected-realm');
  game.check('unaffected entrance still reaches its original independent Realm',otherRealm.view.session.world===1);
  await escape(game);const final=await save('final'),otherFinal=await save('other-final',watcher);
  game.check('journey preserves living identities inventory and durable inventory revision',final.view.lifecycle.alive&&otherFinal.view.lifecycle.alive&&final.view.session.account===initial.view.session.account&&final.view.session.character===initial.view.session.character&&JSON.stringify(final.view.character.items)===JSON.stringify(initial.view.character.items)&&final.view.character.inventory_revision===initial.view.character.inventory_revision&&otherFinal.view.session.character===unaffected.view.session.character);
}
