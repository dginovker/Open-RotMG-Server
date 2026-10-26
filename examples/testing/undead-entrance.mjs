export const fixture = {
  class:0x300, level:20, seed:12351, world:'undead-lair', worldSeed:12351,
  position:'spawn', equipment:[-1,-1,-1,-1], backpack:Array(8).fill(-1),
  stats:[720,252,50,25,75,75,40,50],
};
export async function run(game) {
  const before=await game.observe();
  game.check('starts at the prepared Undead Lair entrance',before.world==='Undead Lair');
  await game.act({keys:[],duration_ms:1500});
  await game.act({keys:[],duration_ms:1500},{name:'entrance',when:v=>v.lifecycle.alive});
  await game.act({keys:['d'],duration_ms:400});
  await game.act({keys:[],duration_ms:1500},{name:'entrance-east',when:v=>v.lifecycle.alive});
  await game.act({keys:['a'],duration_ms:800});
  await game.act({keys:[],duration_ms:1500},{name:'entrance-west',when:v=>v.lifecycle.alive});
  const after=await game.observe();
  game.check('native walking changes position',Math.hypot(after.position.x-before.position.x,after.position.y-before.position.y)>.2);
  game.check('entrance remains safe',after.lifecycle.alive&&after.character.hp===before.character.hp);
  game.check('same character and dungeon',after.session.character===before.session.character&&after.session.world===before.session.world);
}
