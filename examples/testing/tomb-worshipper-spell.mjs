import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';

export const fixture={class:0x310,level:20,seed:12351,world:'tomb-of-the-ancients',worldSeed:12,position:{x:145.1,y:112.5},equipment:[0xa04,0xc1e,0xb05,0xacd],backpack:Array(8).fill(-1),stats:[670,385,50,25,50,50,40,75]};
const wizard={...fixture,class:0x30e,position:{x:146.8,y:112.5},equipment:[0xb08,0xad5,0xb05,0xacd],stats:[670,385,75,25,50,75,40,60]};
export async function run(game){
  // A template name creates another dungeon; join the primary's observed instance without a new seed.
  const mage=await game.join({...wizard,world:(await game.observe()).session.world,worldSeed:undefined}),peers=[game,mage],out=process.env.QA_RESULTS_DIR,snapshots=[],samples=[],casts=[],healing=[];let target,sarc,latest,players=[],failure;
  const save=async label=>{const party=await Promise.all(peers.map(async g=>({actor:g.actor,observed:await g.observe(),inspected:await g.inspect()})));const s={label,party};snapshots.push(s);return s;};
  const safe=s=>s.party.forEach(p=>assert(p.observed.lifecycle.alive&&p.inspected.view.character.hp>300&&p.observed.game_ms<26000,'Living party health-above300 and exposure-below26000 required'));
  const npc=s=>s.diagnostic.entities.find(e=>e.id===target);
  async function events(){
    const text=await readFile(join(out,'native/server.log'),'utf8');assert(text.endsWith('\n'),'Ability log must contain complete records');
    const accepted=text.trimEnd().split('\n').filter(line=>line.startsWith('{')).map(JSON.parse).filter(r=>r.fields?.message==='ability activated'&&players.includes(r.fields.player)).map(r=>r.fields);casts.splice(0,casts.length,...accepted);return accepted;
  }
  async function sample(){const [s,p]=await Promise.all([mage.inspect(),game.inspect()]),e=npc(s);assert(e&&e.hp>0,'Original living priest disappeared');for(const actor of [s,p])assert(actor.view.lifecycle.alive&&actor.view.character.hp>300&&actor.view.time_ms<26000,'Observed party health/exposure guard failed');samples.push({mage:s,priest:p});latest={stage:e.stage,time:s.view.time_ms};return e;}
  async function watched(input,capture){
    let active=true;const action=game.act(input,capture).finally(()=>active=false);
    const observing=(async()=>{for(let n=0;active&&(await game.observe()).paused;n++){assert(n<250,'Native party did not unpause');await delay(20);}while(active){await sample();await delay(80);}})();
    const results=await Promise.allSettled([action,observing]);for(const r of results)if(r.status==='rejected')throw r.reason;
  }
  async function ready(g,cost){
    let s=await save('ability-ready');safe(s);let p=s.party.find(p=>p.actor===g.actor).inspected.view,remaining=p.character.ability_ready_ms-p.time_ms;
    assert(Number.isSafeInteger(p.character.ability_ready_ms)&&Number.isSafeInteger(p.time_ms),'Current simulation-domain readiness required');
    if(remaining>0){assert(remaining<=500,'Unexpected selected ability cooldown');await watched([{actor:g.actor,input:{keys:[],duration_ms:remaining+50}}]);s=await save('cooldown-settled');safe(s);p=s.party.find(p=>p.actor===g.actor).inspected.view;}
    assert(p.character.ability_ready_ms<=p.time_ms&&p.character.mp>=cost,'Finite mana and actual ability readiness required');return s;
  }
  async function tome(initial=false){
    const before=await save('tome-ready');safe(before);if(!initial&&before.party[0].inspected.view.character.hp>600&&before.party[1].inspected.view.character.hp>560)return;
    const prior=(await events()).filter(e=>e.player===players[0]);assert(prior.length<4,'Four finite Tome uses exhausted');await ready(game,100);
    await watched([{actor:game.actor,input:{keys:[' '],duration_ms:100}}]);const after=await save('tome-used');safe(after);const accepted=(await events()).filter(e=>e.player===players[0]);
    assert(accepted.length===prior.length+1&&accepted.every(e=>e.item===0xc1e&&e.mana_cost===100&&e.cooldown_ms===500),'Tome must be accepted exactly once with selected costs');healing.push({before:before.party.map(p=>p.inspected.view.character.hp),after:after.party.map(p=>p.inspected.view.character.hp)});
  }
  try{
    const before=await save('prepared');safe(before);players=before.party.map(p=>p.inspected.diagnostic.player);
    assert(new Set(peers.map(g=>g.actor)).size===2&&before.party.every(p=>p.observed.world==='Tomb of the Ancients'&&Number.isSafeInteger(p.observed.session.world)&&p.observed.session.world===before.party[0].observed.session.world&&!p.inspected.diagnostic.entities.some(e=>e.kind===0xd1a)),'Two fresh same-Tomb actors outside Mummy encounters required');
    const original=before.party[1].inspected.diagnostic.entities.find(e=>e.kind===0xd29&&Math.hypot(e.pos.x-144.5,e.pos.y-112.5)<.1);assert(original&&original.hp===15000,'Original full-health Sarcophagus missing');sarc=original.id;
    const guards=before.party[1].inspected.diagnostic.entities.filter(e=>[0xd2a,0xd2b].includes(e.kind)&&e.tomb?.anchor===sarc);assert(guards.length===8&&guards.every(e=>e.hp===(e.kind===0xd2a?8000:7000)),'Eight full-health original guards required');
    target=guards.filter(e=>e.kind===0xd2a).sort((a,b)=>Math.hypot(a.pos.x-wizard.position.x,a.pos.y-wizard.position.y)-Math.hypot(b.pos.x-wizard.position.x,b.pos.y-wizard.position.y))[0]?.id;assert(target,'Original male priest missing');
    await tome(true);await watched({keys:[],duration_ms:100},{name:'tomb-spell-worshipper-normal',when:v=>v.lifecycle.alive});const normal=await save('normal');safe(normal);
    assert(normal.party[0].inspected.view.character.conditions&(1<<25),'Ordinary Tome must apply self Armored, not invulnerability');assert(npc(normal.party[1].inspected).hp===8000,'Normal priest must remain undamaged');
    for(let n=0;n<7;n++){
      await tome();await ready(mage,60);const e=await sample();if([1,2].includes(e.stage))break;assert(e.stage===0&&e.hp>=2400,'Damage requires the same normal priest above its strict retreat threshold');
      const s=await save('cast-ready');safe(s);assert(Math.hypot(e.pos.x-s.party[1].inspected.view.position.x,e.pos.y-s.party[1].inspected.view.position.y)<5.4,'Nova target must avoid the SDK direction clamp');const prior=(await events()).filter(e=>e.player===players[1]);
      // Fund projectile expiry before another cast; do not stack unobserved damage on a low-health guard.
      await watched([{actor:mage.actor,input:{keys:[' '],duration_ms:100,target:{entity:target,camera_angle:-Math.PI/4,fire:false}}},{actor:mage.actor,input:{keys:[],duration_ms:900}}]);
      const after=await save('cast-settled');safe(after);assert(after.party[1].observed.game_ms-s.party[1].observed.game_ms>=1000,'A full simulation second must settle the nova');const accepted=(await events()).filter(e=>e.player===players[1]);assert(accepted.length===prior.length+1&&accepted.every(e=>e.item===0xad5&&e.mana_cost===60&&e.cooldown_ms===500),'One selected spell activation per request required');
    }
    const black=await save('black-ready');safe(black);assert([1,2].includes((await sample()).stage),'Seven bounded spells did not reach retreat/healing');
    await watched({keys:[],duration_ms:100},{name:'tomb-spell-worshipper-black',when:v=>Boolean(latest&&[1,2].includes(latest.stage)&&v.lifecycle.alive&&v.game_ms>=latest.time)});
    for(let n=0;n<6;n++){const e=await sample();if(e.stage===3)break;assert([1,2].includes(e.stage),'Unexpected healing phase');await tome();await watched({keys:[],duration_ms:1000});safe(await save('healing-wait'));}
    const yellow=await save('yellow-ready');safe(yellow);const e=await sample();assert(e.stage===3&&e.size===120,'Same priest must naturally reach enlarged yellow orbit');
    await watched({keys:[],duration_ms:100},{name:'tomb-spell-worshipper-yellow',when:v=>v.lifecycle.alive});
    await game.act([peers.map(g=>({actor:g.actor,input:{keys:['Insert'],duration_ms:300}}))]);const escaped=await save('escaped');safe(escaped);await events();const phases=samples.map(s=>npc(s.mage));
    game.check('finite ordinary nova damage triggers original priest retreat',phases.some(e=>e.stage===1&&e.hp<2400));
    game.check('same priest heals to full and enlarges without further damage',phases.some(e=>e.stage===2&&e.hp===8000)&&phases.some(e=>e.stage===3&&e.size===120));
    game.check('ordinary finite Tome has an observed health-increase witness',healing.some(h=>h.after.some((hp,i)=>hp>h.before[i])));
    game.check('original Sarcophagus survives and no character earns kills or Tomb credit',snapshots.filter(s=>s.label!=='escaped').every(s=>s.party[1].inspected.diagnostic.entities.some(e=>e.id===sarc&&e.hp===15000))&&snapshots.every(s=>s.party.every(p=>p.inspected.diagnostic.character.kills===0&&p.inspected.diagnostic.character.tombs===0)));
    game.check('both same living legal characters return with unchanged gear and no primary fire',escaped.party.every((p,i)=>p.observed.world==='Nexus'&&p.observed.session.character===before.party[i].observed.session.character&&JSON.stringify(p.inspected.view.character.items)===JSON.stringify(before.party[i].inspected.view.character.items)&&p.inspected.diagnostic.character.shots===(i===0?0:20*casts.filter(e=>e.player===players[1]).length)));
  }catch(error){failure=error;throw error;}finally{
    let auditError;try{await events();}catch(error){auditError=error;}
    await writeFile(join(out,'worshipper-spell.json'),JSON.stringify({qualification:'Two genuine prepared native clients with selected legal gear/base stats and supplied seed12 positions, not earned entry. SDK-only keys/target aiming, no primary fire, bots, protection/stat/NPC/HP/RNG edits, refill, relocation, revival or adoption. Holy Protection is finite ordinary self Armored and healing, not invulnerability. Snapshot polling/capture is non-atomic; holds are not total exposure. Current reconstructed retreat/heal/yellow phases only, not historical healing amount/cadence or full dungeon acceptance.',snapshots,samples,casts,healing,error:failure?String(failure):null,auditError:auditError?String(auditError):null},null,2));
    if(auditError)throw new AggregateError([...(failure?[failure]:[]),auditError],'Native case and final accepted-input audit must both be observable');
  }
}
