import assert from 'node:assert/strict';
import {readFile, appendFile, readdir, writeFile} from 'node:fs/promises';
import {randomUUID, createHash} from 'node:crypto';

export const fixture = {
  class:0x310, level:20, seed:12351, world:'ocean-trench', position:'spawn',
  equipment:[0xaf6,0xb25,0xb05,0xac5], backpack:Array(8).fill(0xa22),
  stats:[670,385,50,25,50,50,40,75],
};
export async function run(game) {
  const out=process.argv[process.argv.indexOf('--run-dir')+1];
  const config=JSON.parse(await readFile(out+'/native.json','utf8'));
  const token=(await readFile(config.state+'/control.token','utf8')).trim();
  const selection=JSON.parse(await readFile(config.state+'/client-selection.json','utf8'));
  const client=config.state+'/Open-RotMG-Client-'+selection.client,served={};
  for(const path of ['/client.swf','/launcher.js','/index.html',...(await readdir(client+'/vendor')).filter(p=>/\.(js|wasm)$/.test(p)).map(p=>'/vendor/'+p)]) {
    const response=await fetch(config.origin+path);assert(response.ok,path);
    const bytes=Buffer.from(await response.arrayBuffer());
    served[path]={bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')};
  }
  await writeFile(out+'/client-binding.json',JSON.stringify({selection,served,lock:JSON.parse(await readFile(client+'/ruffle/lock.json','utf8')),originalSwfSha256:createHash('sha256').update(await readFile(client+'/client.swf')).digest('hex')},null,2)+'\n');
  async function inspect(label) {
    const response=await fetch(config.origin+'/dev/testing',{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify({id:'oxygen-'+randomUUID(),command:{op:'inspect',actor:game.actor}})});
    const value=await response.json();assert(response.ok&&value.diagnostic,JSON.stringify(value));
    if(label)await appendFile(out+'/oxygen.jsonl',JSON.stringify({label,...value.diagnostic})+'\n');
    return value.diagnostic;
  }
  async function captureOxygen(label,predicate) {
    const d=await inspect();
    if(!predicate(d.oxygen))return false;
    await appendFile(out+'/oxygen.jsonl',JSON.stringify({label,...d})+'\n');
    return true;
  }
  const start=await inspect('spawn');
  game.check('Ocean Trench starts at 100 oxygen',start.oxygen===100);
  await game.act({keys:[],duration_ms:900},{name:'oxygen-full',when:()=>captureOxygen('full-capture-condition',oxygen=>oxygen===100)});
  const spawn=start.pos;
  await game.act({keys:['a','s'],duration_ms:500});
  const dry=await inspect('dry-start');
  game.check('dry position is outside every nearby vent',dry.entities.filter(e=>e.kind===0x731).every(e=>Math.hypot(e.pos.x-dry.pos.x,e.pos.y-dry.pos.y)>1.2));
  const travel=dry.pos.y-spawn.y;
  game.check('default 7pi/4 camera cardinal pair moves south',Math.abs(dry.pos.x-spawn.x)<0.5&&travel>1.2);
  await game.act({keys:[],duration_ms:Math.round(dry.oxygen*75+300)},{name:'oxygen-low',when:()=>captureOxygen('low-capture-condition',oxygen=>oxygen>0&&oxygen<20)});
  const empty=await inspect('empty-after-nonlethal-drowning');
  game.check('oxygen naturally empties and living actor loses HP',empty.oxygen===0&&empty.character.hp<dry.character.hp&&empty.character.alive);
  await game.act({keys:['d','w'],duration_ms:500},{name:'oxygen-empty',when:()=>captureOxygen('empty-capture-condition',oxygen=>oxygen===0)});
  const pocket=await inspect('returned-to-spawn-pocket');
  game.check('ordinary walk reaches the measured spawn vent',Math.hypot(pocket.pos.x-spawn.x,pocket.pos.y-spawn.y)<0.8);
  await game.act({keys:[],duration_ms:2200},{name:'oxygen-recovered',when:()=>captureOxygen('recovered-capture-condition',oxygen=>oxygen===100)});
  const recovered=await inspect('recovered');
  game.check('vent refills oxygen to cap without continued suffocation',recovered.oxygen===100&&recovered.character.hp>=pocket.character.hp);
  const corridor=spawn.x===12.5?{x:18.5,y:12.5}:null;
  assert(corridor,'Unexpected prepared spawn; corridor case needs the selected generated layout');
  await game.act([
    {actor:game.actor,input:{keys:['d','s'],duration_ms:Math.round(500*5.7/travel)}},
    {actor:game.actor,input:{keys:[],duration_ms:1800}},
  ],{name:'oxygen-corridor-pocket',when:async()=>{const d=await inspect();return Math.hypot(d.pos.x-corridor.x,d.pos.y-corridor.y)<0.8&&d.oxygen===100;}});
  const end=await inspect('corridor-pocket');
  game.check('normal short corridor traversal reaches a different vent',end.oxygen===100&&Math.hypot(end.pos.x-corridor.x,end.pos.y-corridor.y)<0.8&&Math.hypot(end.pos.x-spawn.x,end.pos.y-spawn.y)>5);
}
