import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';
const spec={class:0x307,level:20,seed:785,world:-3,position:'spawn',equipment:[0xc02,-1,-1,-1],backpack:Array(8).fill(-1),stats:null};
let world;
const arrival=index=>({x:world.spawn_points[index][0],y:world.spawn_points[index][1]});
export async function prepare(api){
  world=await api({op:'world_info',world:-3});
  if(world.spawn_points.length!==12||!world.realm_layout)throw Error('Expected twelve arrivals in a registered Realm');
  await writeFile(join(process.env.QA_RESULTS_DIR,'realm.json'),JSON.stringify(world));
  return {...spec,position:arrival(0)};
}
export async function run(game){
  const initial=await game.inspect(),peer=await game.join({...spec,seed:786,position:arrival(0)});
  await game.act({keys:[],duration_ms:500});
  const peerInitial=await peer.observe(),firstVisible=await game.observe();
  await writeFile(join(process.env.QA_RESULTS_DIR,'shared-arrival.json'),JSON.stringify({first:firstVisible,peer:peerInitial}));
  game.check('prepared native players share the same Realm',initial.view.session.world===peerInitial.session.world);
  game.check('first arrival retains its registered coastal position',Math.hypot(initial.view.position.x-arrival(0).x,initial.view.position.y-arrival(0).y)<.1);
  game.check('both clients see the other player',firstVisible.nearby.some(o=>o.id===peerInitial.session.player)&&peerInitial.nearby.some(o=>o.id===initial.view.session.player));
  await game.act({keys:['d','w'],duration_ms:200});
  await game.act({keys:[],duration_ms:500},{name:'realm-coastal-arrival',when:()=>true});
  const moved=await game.observe(),watching=await peer.inspect();
  game.check('native input leaves the southern arrival inward',moved.position.y<initial.view.position.y-.5);
  const visible=watching.view.objects.find(o=>o.id===moved.session.player);
  game.check('second client receives the first player movement',!!visible&&Math.hypot(visible.position.x-moved.position.x,visible.position.y-moved.position.y)<.2);
  const remote=await game.join({...spec,seed:787,position:arrival(2)});
  const remoteInitial=await remote.observe();
  game.check('opposite arrival enters the same living Realm',remoteInitial.session.world===initial.view.session.world&&Math.hypot(remoteInitial.position.x-arrival(2).x,remoteInitial.position.y-arrival(2).y)<.1);
  await remote.act({keys:['a','s'],duration_ms:200});
  await remote.act({keys:[],duration_ms:500},{name:'realm-opposite-arrival',when:()=>true});
  const remoteMoved=await remote.observe();
  game.check('native input leaves the northern arrival inward',remoteMoved.position.y>remoteInitial.position.y+.5);
  for(const client of [game,peer,remote])game.check('prepared coastal character remains alive',(await client.observe()).lifecycle.alive);
}
