export const fixture = {
  class:0x30e, level:20, seed:12351, world:-2, position:'spawn',
  equipment:[-1,0xa2e,-1,-1], backpack:[2595,-1,-1,-1,-1,-1,-1,-1], stats:null,
};
export async function run(game) {
  const before=await game.observe();
  game.check('prepared Magic Potion occupies slot 4',before.character.items[4]===2595);
  await game.act({keys:[' '],duration_ms:250});
  const spent=await game.observe();
  game.check('ordinary native spell spends mana',spent.character.mp<before.character.mp);
  await game.act({keys:['1'],duration_ms:1500},{name:'item-used',when:view=>view.character.items[4]===-1});
  const after=await game.observe();
  game.check('potion consumed once',after.character.items[4]===-1&&after.character.inventory_revision===spent.character.inventory_revision+1);
  game.check('potion restored mana',after.character.mp>spent.character.mp);
  game.check('same living character',after.lifecycle.alive&&after.session.character===before.session.character);
}
