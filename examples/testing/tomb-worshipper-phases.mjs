import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {connect} from '../../scripts/record-demo.mjs';

export const fixture={class:0x31e,level:20,seed:12351,world:'tomb-of-the-ancients',worldSeed:12,position:{x:143.4,y:113.1},equipment:[0xb0b,0xb22,0xafc,0xac8],backpack:Array(8).fill(-1),stats:[770,252,50,40,50,50,75,50]};
export async function run(game){
  const out=process.env.QA_RESULTS_DIR,{worldSeed,...spec}=fixture,start=await game.observe();
  const peers=[game,await game.join({...spec,world:start.session.world,position:{x:143.6,y:113.1}}),await game.join({...spec,class:0x310,world:start.session.world,position:{x:150.4,y:113.4},equipment:[0xaf6,0xb25,0xb05,0xac8],stats:[670,385,50,25,50,50,40,75]})];
  const snapshots=[],samples=[],pointers=[];let connection,latest;
  const save=async label=>{const actors=await Promise.all(peers.map(async g=>({actor:g.actor,observed:await g.observe(),inspected:await g.inspect()})));snapshots.push({label,actors});return actors;};
  const group=(ms,heal,fire)=>peers.map((g,n)=>({actor:g.actor,input:{keys:n===2&&heal?[' ']:[],duration_ms:n===0?ms:100,...(n===0&&fire?{target:{entity:fire,camera_angle:-Math.PI/4,fire:true}}:{})}}));
  try{
    const before=await save('prepared');
    assert(before.every(s=>s.observed.world==='Tomb of the Ancients'&&s.observed.lifecycle.alive),'Fresh living Tomb party required');
    assert(before.every(s=>!s.inspected.diagnostic.entities.some(e=>e.kind===0xd1a)),'Mummy encounters are outside this component scope');
    const caster=before[2].inspected.view.position;
    assert(before.slice(0,2).every(s=>Math.hypot(s.inspected.view.position.x-caster.x,s.inspected.view.position.y-caster.y)<8),'Both Knights must start within the selected Tome range');
    await game.act([group(2000,false)],{name:'tomb-worshipper-normal',when:v=>v.lifecycle.alive&&v.game_ms>=1500});
    const normal=await save('normal'),state=normal[0].inspected;
    assert((normal[0].inspected.view.character.conditions&4)!==0,'Ordinary incoming Weak required before bounded sword observation');
    const sarc=state.diagnostic.entities.find(e=>e.kind===0xd29&&Math.hypot(e.pos.x-144.5,e.pos.y-112.5)<0.1);assert(sarc,'Original prepared Sarcophagus missing');
    const guards=state.diagnostic.entities.filter(e=>[0xd2a,0xd2b].includes(e.kind)&&e.tomb?.anchor===sarc.id);
    assert(guards.length===8&&guards.every(e=>e.hp===(e.kind===0xd2a?8000:7000)),'Eight undamaged original worshippers required');
    const target=guards.filter(e=>e.kind===0xd2a).sort((a,b)=>Math.hypot(a.pos.x-state.view.position.x,a.pos.y-state.view.position.y)-Math.hypot(b.pos.x-state.view.position.x,b.pos.y-state.view.position.y))[0];assert(target,'Original male worshipper missing');
    assert(normal.every(s=>s.inspected.view.character.hp>300),'Party health insufficient for the bounded exposure');
    const config=JSON.parse(await readFile(join(out,'native/native.json'),'utf8')),browser=config.native.find(n=>n.actor===game.actor);assert(browser,'Owned lead browser missing');
    connection=await connect({...browser,url:config.origin+'/',helper:config.browserTools});
    const rect=await connection.eval('(()=>{const r=document.querySelector("ruffle-player").getBoundingClientRect();return{x:r.x,y:r.y,width:r.width,height:r.height};})()');assert(rect.width===800&&rect.height===600,'Native player must be unscaled');
    async function sample(){const frame=await game.inspect(),entity=frame.diagnostic.entities.find(e=>e.id===target.id);assert(entity,'Original worshipper disappeared');latest={time:frame.view.time_ms,stage:entity.stage};samples.push(frame);return{frame,entity};}
    async function tracked(inputs,capture){
    let active=true;
    const action=game.act(inputs,capture).finally(()=>active=false);
    const tracking=(async()=>{
      for(let i=0;active&&(await game.observe()).paused;i++){assert(i<250,'Native party did not unpause');await delay(20);}
      let previous;
      while(active){
        const {frame}=await sample();if(!active)break;
        const body=frame.view.objects.find(e=>e.id===target.id&&e.hp>0);assert(body,'Original worshipper left the native neighborhood');
        if(previous&&frame.view.time_ms>previous.time){
          const dt=(frame.view.time_ms-previous.time)/1000,vx=(body.position.x-previous.pos.x)/dt,vy=(body.position.y-previous.pos.y)/dt;
          // Native shot traces used aim sampled100–150ms earlier, in addition to projectile flight.
          const t=0.15+Math.min(0.35,Math.hypot(body.position.x-frame.view.position.x,body.position.y-frame.view.position.y)/10),dx=body.position.x+vx*t-frame.view.position.x,dy=body.position.y+vy*t-frame.view.position.y,a=-Math.PI/4;
          const x=50*(dx*Math.cos(a)+dy*Math.sin(a)),y=50*(-dx*Math.sin(a)+dy*Math.cos(a)),scale=Math.max(1,Math.hypot(x,y)/270),mouse={x:rect.x+300+x/scale,y:rect.y+325+y/scale};
          assert(Number.isFinite(mouse.x)&&Number.isFinite(mouse.y),'Nonfinite native aim');
          await connection.call('Input.dispatchMouseEvent',{type:'mouseMoved',buttons:1,...mouse});pointers.push({time:frame.view.time_ms,target:body.position,mouse});
        }
        previous={time:frame.view.time_ms,pos:body.position};await delay(80);
      }
    })();
    const settled=await Promise.allSettled([action,tracking]);for(const r of settled)if(r.status==='rejected')throw r.reason;
    }
    await tracked(Array.from({length:2},()=>group(3000,true,target.id)));
    const primedParty=await save('primed'),primed=primedParty[0].inspected.diagnostic.entities.find(e=>e.id===target.id);
    assert(primed?.stage===0&&primed.hp>2400&&primed.hp<=6000,'Ordinary primer must leave the same priest in stage0 with2400<HP<=6000');
    assert(primedParty[2].inspected.view.character.mp<normal[2].inspected.view.character.mp&&samples.some(s=>s.view.character.hp>=normal[0].inspected.view.character.hp+50),'Ordinary ranged Tome must spend mana and restore observed Knight health');
    assert(primedParty.every(s=>s.observed.lifecycle.alive&&s.inspected.view.character.hp>300),'Party health abort before sustained fire');
    await tracked(Array.from({length:3},()=>group(3000,true,target.id)),{name:'tomb-worshipper-black-phase',when:v=>Boolean(latest&&[1,2].includes(latest.stage)&&v.lifecycle.alive&&v.game_ms>=latest.time)});
    await save('after-fire');
    for(let n=0;n<6;n++){
      const {entity}=await sample();if(entity.stage===3)break;
      const party=await save('healing-wait-'+n);assert(party.every(s=>s.observed.lifecycle.alive&&s.inspected.view.character.hp>300),'Party health abort before another exposure');
      assert(party[0].observed.game_ms<26000,'Worshipper transition exposure exhausted');
      await game.act([group(1000,party[2].inspected.view.character.mp>=85)]);
    }
    const {entity}=await sample();assert(entity.stage===3,'Original worshipper did not reach its yellow enlarged orbit');
    const party=await save('yellow-ready');assert(party.every(s=>s.observed.lifecycle.alive&&s.inspected.view.character.hp>300),'Party health abort before capture');
    await game.act([group(1000,party[2].inspected.view.character.mp>=85)],{name:'tomb-worshipper-yellow-orbit',when:v=>v.lifecycle.alive});
    await save('yellow');
    await game.act([peers.map(g=>({actor:g.actor,input:{keys:['Insert'],duration_ms:300}}))]);
    const escaped=await save('escaped'),stages=samples.map(s=>s.diagnostic.entities.find(e=>e.id===target.id));
    game.check('ordinary sword damage triggers the original priest retreat',stages.some(e=>e.stage===1&&e.hp<2400)&&escaped[0].inspected.diagnostic.character.shots>0);
    game.check('same priest naturally heals to full health then enlarges',stages.some(e=>e.stage===2&&e.hp===8000)&&stages.some(e=>e.stage===3&&e.size===120));
    game.check('prepared Sarcophagus survives without awakening bosses',snapshots.filter(s=>s.label!=='escaped').every(s=>s.actors[0].inspected.diagnostic.entities.some(e=>e.id===sarc.id&&e.hp>0)&&!s.actors[0].inspected.diagnostic.entities.some(e=>[0xd26,0xd27,0xd28].includes(e.kind))));
    game.check('same three living characters escape without kills or Tomb credit',escaped.every((s,n)=>s.observed.world==='Nexus'&&s.observed.lifecycle.alive&&s.observed.session.character===before[n].observed.session.character&&s.inspected.diagnostic.character.kills===0&&s.inspected.diagnostic.character.tombs===0));
  }finally{connection?.close();await writeFile(join(out,'worshipper-phases.json'),JSON.stringify({qualification:'Prepared component test, two legal max-stat Knights plus one Priest with a supplied remote caster and ordinary selected300HealNova/range8/85MP Tome. No NPC/HP/RNG edits, injected protection, support bots, refill or relocation. One sword fires under the native lease with recorded owned-page pointer tracking; input holds are not total exposure. Current reconstruction phases, not historical heal-rate/AI/full-dungeon acceptance.',snapshots,samples,pointers},null,2));}
}
