import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';

export const fixture={class:0x300,level:20,seed:12351,world:'tomb-of-the-ancients',worldSeed:12,position:{x:144.5,y:112.5},equipment:[0xc0a,0xae1,0xaf9,0xacd],backpack:Array(8).fill(-1),stats:[720,252,50,25,75,75,40,50]};
export async function run(game){
  const out=process.env.QA_RESULTS_DIR,snapshots=[],samples=[],casts=[];let target,latest,player;
  const save=async label=>{const s={label,observed:await game.observe(),inspected:await game.inspect()};snapshots.push(s);return s;};
  const safe=s=>assert(s.observed.lifecycle.alive&&s.inspected.view.character.hp>300&&s.observed.game_ms<26000,'Living health-above300 and exposure-below26000 required');
  async function uses(){
    const text=await readFile(join(out,'native/server.log'),'utf8');
    assert(text.endsWith('\n'),'Native ability log must contain complete records');
    const accepted=text.trimEnd().split('\n').filter(line=>line.startsWith('{')).map(JSON.parse).filter(r=>r.fields?.message==='ability activated'&&r.fields.player===player&&r.fields.item===0xae1).map(r=>r.fields);casts.splice(0,casts.length,...accepted);return accepted;
  }
  async function cloak(s){
    const log=await uses(),last=log.at(-1);assert(last&&last.mana_cost===85&&last.cooldown_ms===6500,'Accepted selected cloak event required');
    const ready=s.inspected.view.character.ability_ready_ms,time=s.inspected.view.time_ms;
    assert(Number.isSafeInteger(ready)&&Number.isSafeInteger(time),'Simulation-domain cloak readiness missing from native view');
    const remaining=ready-time;
    return {keys:remaining<=0&&s.inspected.view.character.mp>=85?[' ']:[],near:remaining>0&&remaining<1500,count:log.length};
  }
  async function sample(){const s=await game.inspect(),e=s.diagnostic.entities.find(e=>e.id===target);assert(e,'Original worshipper disappeared');samples.push(s);latest={stage:e.stage,time:s.view.time_ms};return e;}
  async function watched(input,capture){
    let active=true;const action=game.act(input,capture).finally(()=>active=false);
    const observing=(async()=>{for(let n=0;active&&(await game.observe()).paused;n++){assert(n<250,'Native actor did not unpause');await delay(20);}while(active){await sample();await delay(80);}})();
    const results=await Promise.allSettled([action,observing]);for(const r of results)if(r.status==='rejected')throw r.reason;
  }
  try{
    const before=await save('prepared');player=before.inspected.diagnostic.player;
    assert(before.observed.world==='Tomb of the Ancients'&&before.observed.lifecycle.alive,'Fresh living Tomb actor required');
    assert(!before.inspected.diagnostic.entities.some(e=>e.kind===0xd1a),'Mummy encounters are outside this component scope');
    // The opening volley is already in flight before cloak acceptance; cross its old aim lines.
    await game.act([{actor:game.actor,input:{keys:['w',' '],duration_ms:100}},{actor:game.actor,input:{keys:['a'],duration_ms:100}},{actor:game.actor,input:{keys:[],duration_ms:1600}}],{name:'tomb-worshipper-normal',when:v=>v.lifecycle.alive&&v.game_ms>=1500});
    const normal=await save('normal');safe(normal);
    assert(normal.inspected.view.character.mp<before.inspected.view.character.mp&&(normal.inspected.view.character.conditions&4096)!==0,'Ordinary cloak must spend mana and apply Invisible, not protection');
    const sarc=normal.inspected.diagnostic.entities.find(e=>e.kind===0xd29&&Math.hypot(e.pos.x-144.5,e.pos.y-112.5)<0.1);assert(sarc,'Original Sarcophagus missing');
    const guards=normal.inspected.diagnostic.entities.filter(e=>[0xd2a,0xd2b].includes(e.kind)&&e.tomb?.anchor===sarc.id);
    assert(guards.length===8&&guards.every(e=>e.hp===(e.kind===0xd2a?8000:7000)),'Eight undamaged original worshippers required');
    target=guards.filter(e=>e.kind===0xd2a).sort((a,b)=>Math.hypot(a.pos.x-normal.inspected.view.position.x,a.pos.y-normal.inspected.view.position.y)-Math.hypot(b.pos.x-normal.inspected.view.position.x,b.pos.y-normal.inspected.view.position.y))[0]?.id;assert(target,'Original male worshipper missing');
    for(let n=0;n<16;n++){
      const s=await save('damage-ready-'+n);safe(s);const e=await sample();if([1,2].includes(e.stage))break;
      assert(e.stage===0,'Original priest passed the black phase before capture');
      const c=await cloak(s),count=c.near?1:4;
      const sequence=Array.from({length:count},(_,i)=>({actor:game.actor,input:{keys:i===0&&c.keys.length?c.keys:c.near?[n%2?'a':'d']:[],duration_ms:c.near?100:250,target:{entity:target,camera_angle:-Math.PI/4,fire:true}}}));
      await watched(sequence);
      if(c.keys.length)assert((await uses()).length===c.count+1,'Requested cloak was not accepted exactly once');
    }
    const black=await save('black-ready');safe(black);assert([1,2].includes((await sample()).stage),'Bounded ordinary fire did not reach the black phase');
    await watched({keys:[],duration_ms:100},{name:'tomb-worshipper-black-phase',when:v=>Boolean(latest&&[1,2].includes(latest.stage)&&v.lifecycle.alive&&v.game_ms>=latest.time)});
    for(let n=0;n<6;n++){
      const e=await sample();if(e.stage===3)break;assert([1,2].includes(e.stage),'Unexpected priest healing phase');
      const s=await save('healing-wait-'+n);safe(s);const c=await cloak(s);
      await watched({keys:c.keys,duration_ms:1000});
      if(c.keys.length)assert((await uses()).length===c.count+1,'Requested waiting cloak was not accepted exactly once');
    }
    const yellow=await save('yellow-ready');safe(yellow);const e=await sample();assert(e.stage===3&&e.size===120,'Original priest did not reach enlarged yellow orbit');
    await watched({keys:[],duration_ms:100},{name:'tomb-worshipper-yellow-orbit',when:v=>v.lifecycle.alive});
    await save('yellow');await game.act({keys:['Insert'],duration_ms:300});
    const escaped=await save('escaped'),stages=samples.map(s=>s.diagnostic.entities.find(e=>e.id===target));
    game.check('ordinary dagger damage triggers original priest retreat',stages.some(e=>e.stage===1&&e.hp<2400)&&escaped.inspected.diagnostic.character.shots>0);
    game.check('same priest naturally heals to full then enlarges',stages.some(e=>e.stage===2&&e.hp===8000)&&stages.some(e=>e.stage===3&&e.size===120));
    game.check('original Sarcophagus survives without awakening bosses',snapshots.filter(s=>s.label!=='escaped').every(s=>s.inspected.diagnostic.entities.some(e=>e.id===sarc.id&&e.hp>0)&&!s.inspected.diagnostic.entities.some(e=>[0xd26,0xd27,0xd28].includes(e.kind))));
    game.check('same living character escapes without kills or Tomb credit',escaped.observed.world==='Nexus'&&escaped.observed.lifecycle.alive&&escaped.observed.session.character===before.observed.session.character&&escaped.inspected.diagnostic.character.kills===0&&escaped.inspected.diagnostic.character.tombs===0);
  }finally{await writeFile(join(out,'worshipper-phases.json'),JSON.stringify({qualification:'Prepared single-native-client component. Selected legal Rogue base720HP plus140HP ring, ordinary dagger/cloak and bounded WASD cooldown sidesteps. Invisibility is not protection. All input uses existing SDK; observer sends no CDP input. No NPC/HP/RNG edits, bots, refill, teleport or adoption. Input holds are not total exposure. Current reconstruction phases, not historical healing-rate/AI/full-dungeon acceptance.',snapshots,samples,casts},null,2));}
}
