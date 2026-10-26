import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';

export const fixture = {
  class:0x310, level:20, seed:12351, world:'snake-pit', worldSeed:12350,
  position:world=>({x:world.spawn_points[0][0],y:world.spawn_points[0][1]-4}),
  equipment:[0xaf6,-1,-1,-1], backpack:[-1,-1,-1,-1,-1,-1,-1,-1], stats:[670,385,50,25,50,50,40,75],
};
export async function run(game) {
  await game.act({keys:[],duration_ms:2500});
  const initial=await game.inspect();
  await writeFile(join(process.env.QA_RESULTS_DIR,'initial.json'),JSON.stringify(initial,null,2));
  await game.act({keys:[],duration_ms:1800},{name:'minimap',when:()=>true});
  const view=await game.inspect();
  const known=new Set(view.view.tiles.map(t=>`${t.x},${t.y}`));
  const walls=view.diagnostic.entities.filter(e=>e.kind===0x172);
  game.check('intact Snake Pit tunnel present',walls.length>4);
  const hidden=walls.filter(e=>!known.has(`${Math.floor(e.pos.x)},${Math.floor(e.pos.y)}`));
  game.check('intact walls hide deeper tunnel tiles',hidden.length>0);
  game.check('hidden walls are not streamed as objects',hidden.every(e=>!view.view.objects.some(o=>o.id===e.id)));
  const distance=e=>Math.hypot(e.position.x-view.view.position.x,e.position.y-view.view.position.y);
  const wall=view.view.objects.filter(e=>e.kind===0x172).sort((a,b)=>distance(a)-distance(b))[0];
  game.check('near soft wall is visible',Boolean(wall));
  const before=await game.observe();
  await game.act({keys:[],duration_ms:2000,target:{entity:wall.id,camera_angle:7*Math.PI/4,fire:true}});
  const after=await game.inspect();
  await writeFile(join(process.env.QA_RESULTS_DIR,'dug.json'),JSON.stringify({before:view,after},null,2));
  game.check('ordinary native shots destroy the wall',after.diagnostic.character.shots>view.diagnostic.character.shots&&!after.diagnostic.entities.some(e=>e.id===wall.id));
  game.check('digging reveals new tiles without moving',after.view.tiles.length>view.view.tiles.length&&Math.hypot(after.view.position.x-before.position.x,after.view.position.y-before.position.y)<0.15);
  game.check('same living native character',after.view.lifecycle.alive&&after.view.session.character===initial.view.session.character);
  await game.act({keys:[],duration_ms:1500},{name:'dug',when:()=>true});
}
