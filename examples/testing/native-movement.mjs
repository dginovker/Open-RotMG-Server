export const fixture = {
  class:0x31e, level:20, seed:12351, world:-2, position:'spawn',
  equipment:[0xa00,-1,-1,-1], backpack:Array(8).fill(-1), stats:null,
};
export async function run(game) {
  const before=await game.observe();
  await game.act({keys:['d'],duration_ms:1600},{name:'movement',when:v=>Math.hypot(v.position.x-before.position.x,v.position.y-before.position.y)>2});
  const moved=await game.observe();
  game.check('ordinary native movement advances',Math.hypot(moved.position.x-before.position.x,moved.position.y-before.position.y)>2);
  await game.act({keys:['a','w'],duration_ms:1000});
  const diagonal=await game.observe();
  game.check('diagonal native movement advances',Math.hypot(diagonal.position.x-moved.position.x,diagonal.position.y-moved.position.y)>1);
  await game.act({keys:[],duration_ms:1000});
  const idle=await game.observe();
  game.check('idle remains stationary',Math.hypot(idle.position.x-diagonal.position.x,idle.position.y-diagonal.position.y)<0.05);
  game.check('same living character',idle.lifecycle.alive&&idle.session.character===before.session.character);
  const peer=await game.join({...fixture,class:0x324,seed:12352,position:idle.position,equipment:[0xa14,0xae3,-1,-1]});
  const peerBefore=await peer.observe();
  await peer.act({keys:['d'],duration_ms:500});
  const peerMoved=await peer.observe();
  const watching=await game.observe();
  game.check('second native player moves',Math.hypot(peerMoved.position.x-peerBefore.position.x,peerMoved.position.y-peerBefore.position.y)>1);
  game.check('first client sees the second player',watching.nearby.some(o=>o.id===peerMoved.session.player));
  game.check('second client sees the first player',peerMoved.nearby.some(o=>o.id===watching.session.player));
  await peer.act({keys:[' '],mouse:{x:500,y:300},duration_ms:250});
  const teleported=await peer.observe();
  game.check('ordinary prism teleports and spends mana',Math.hypot(teleported.position.x-peerMoved.position.x,teleported.position.y-peerMoved.position.y)>3&&teleported.character.mp<peerMoved.character.mp);
  await peer.act({keys:['a'],duration_ms:600},{name:'teleport-movement',when:v=>v.lifecycle.alive&&Math.hypot(v.position.x-teleported.position.x,v.position.y-teleported.position.y)>1});
  const resumed=await peer.observe();
  game.check('movement resumes after GOTO acknowledgement',Math.hypot(resumed.position.x-teleported.position.x,resumed.position.y-teleported.position.y)>1);
  game.check('teleported character remains connected and alive',resumed.lifecycle.alive&&resumed.session.character===peerBefore.session.character);
}
