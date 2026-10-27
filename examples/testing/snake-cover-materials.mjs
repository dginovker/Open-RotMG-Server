import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import assert from 'node:assert/strict';
const root=process.argv[process.argv.indexOf('--run-dir')+1];
export const fixture={class:0x310,level:20,seed:633,world:'snake-pit',worldSeed:633,position:'spawn',equipment:[0xa87,-1,-1,-1],backpack:Array(8).fill(-1),stats:[670,385,50,25,50,50,40,75]};
export async function run(game){
  const config=JSON.parse(await readFile(join(root,'native.json'),'utf8'));
  const token=(await readFile(join(config.state,'control.token'),'utf8')).trim();
  const initial=await game.inspect();
  const map=async()=>{
    const response=await fetch(config.origin+'/dev/map?player='+initial.diagnostic.player,{headers:{Authorization:'Bearer '+token},signal:AbortSignal.timeout(10000)});
    assert(response.ok);return response.json();
  };
  const layout=await map(),boss=layout.entities.find(e=>e.kind===0x917);assert(boss);
  const {worldSeed,...priest}=fixture;
  const cover=layout.entities.filter(e=>[0x170,0x172].includes(e.kind)&&Math.abs(e.pos.x-boss.pos.x)<=2&&e.pos.y<boss.pos.y&&e.pos.y>boss.pos.y-12);
  const top=Math.min(...cover.map(e=>e.pos.y));
  const soft=cover.find(e=>e.pos.x===boss.pos.x+1&&e.pos.y===top),hard=cover.find(e=>e.pos.x===boss.pos.x-1&&e.pos.y===top);assert(soft&&hard);
  game=await game.join({...priest,world:initial.view.session.world,position:{x:soft.pos.x,y:top-3}});
  await game.act({keys:[],duration_ms:1500},{name:'cover-intact',when:v=>v.lifecycle.alive});
  await game.act({keys:[],target:{entity:soft.id,camera_angle:-Math.PI/4,fire:true},duration_ms:1200});
  const dug=await map();
  await game.act({keys:[],mouse:{x:300-5*50/Math.sqrt(2),y:325+50/Math.sqrt(2)},duration_ms:1200});
  await game.act({keys:[],duration_ms:1000},{name:'cover-shot',when:v=>v.lifecycle.alive});
  const after=await map();
  await writeFile(join(root,'cover-observations.json'),JSON.stringify({boss,soft,hard,dug,after},null,2));
  game.check('boss cover includes selected 100-HP destructible wall',soft.kind===0x172&&soft.hp===100);
  game.check('native wand fire destroys soft cover',!dug.entities.some(e=>e.id===soft.id));
  game.check('destroyed cover becomes walkable',!dug.blocked.some(([x,y])=>x===Math.floor(soft.pos.x)&&y===Math.floor(soft.pos.y)));
  game.check('permanent cover survives native wand fire',hard.kind===0x170&&after.entities.some(e=>e.id===hard.id));
  game.check('permanent cover stays blocking',after.blocked.some(([x,y])=>x===Math.floor(hard.pos.x)&&y===Math.floor(hard.pos.y)));
  game.check('prepared priest survives', (await game.observe()).lifecycle.alive);
}
