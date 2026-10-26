export const fixture = {
  class:0x31e, level:20, seed:12351, world:'sprite-world', worldSeed:1, position:'spawn',
  equipment:[0xb0b,0xb22,0xafc,0xacd], backpack:Array(8).fill(-1),
  stats:[770,252,50,40,50,50,75,50],
};
export async function run(game) {
  const before=await game.observe();
  game.check('living character in generated Sprite World',before.world==='Sprite World'&&before.lifecycle.alive);
  await game.act({keys:[],duration_ms:1800},{name:'sprite-conveyors',when:v=>v.lifecycle.alive&&v.game_ms>=before.game_ms+800});
  const idle=await game.observe();
  game.check('entrance remains stationary',Math.hypot(idle.position.x-before.position.x,idle.position.y-before.position.y)<0.05);
  await game.act({keys:['d'],duration_ms:400});
  const moved=await game.observe();
  game.check('normal native movement works',Math.hypot(moved.position.x-idle.position.x,moved.position.y-idle.position.y)>0.1);
  game.check('same living character',moved.lifecycle.alive&&moved.session.character===before.session.character);
}
