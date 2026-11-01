import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {setTimeout as delay} from 'node:timers/promises';
import {connect} from '../../scripts/record-demo.mjs';

export const fixture={class:0x31e,level:20,seed:12351,world:'tomb-of-the-ancients',worldSeed:12,position:{x:144.1,y:112.9},equipment:[0xb0b,0xb22,0xafc,0xac8],backpack:Array(8).fill(-1),stats:[770,252,50,40,50,50,75,50]};
export async function run(game){
  const out=process.env.QA_RESULTS_DIR,{worldSeed,...spec}=fixture,start=await game.observe();
  const peers=[game,await game.join({...spec,world:start.session.world,position:{x:144.3,y:112.9}}),await game.join({...spec,class:0x310,world:start.session.world,position:{x:144.2,y:112.8},equipment:[0xaf6,0xc1e,0xb05,0xac8],stats:[670,385,50,25,50,50,40,75]})];
  const snapshots=[],samples=[],pointers=[];let connection,latest;
  const save=async label=>{const actors=await Promise.all(peers.map(async g=>({actor:g.actor,observed:await g.observe(),inspected:await g.inspect()})));snapshots.push({label,actors});return actors;};
  const group=(ms,heal,fire)=>peers.map((g,n)=>({actor:g.actor,input:{keys:n===2&&heal?[' ']:[],duration_ms:ms,...(n===0&&fire?{target:{entity:fire,camera_angle:-Math.PI/4,fire:true}}:{})}}));
  try{
    const before=await save('prepared');
    assert(before.every(s=>s.observed.world==='Tomb of the Ancients'&&s.observed.lifecycle.alive),'Fresh living Tomb party required');
    assert(!before[0].inspected.diagnostic.entities.some(e=>e.kind===0xd1a),'Mummy encounters are outside this component scope');
    await game.act([group(2000,true)],{name:'tomb-worshipper-normal',when:v=>v.lifecycle.alive&&v.game_ms>=1500});
    const normal=await save('normal'),state=normal[0].inspected;
    const sarc=state.diagnostic.entities.find(e=>e.kind===0xd29&&Math.hypot(e.pos.x-144.5,e.pos.y-112.5)<0.1);assert(sarc,'Original prepared Sarcophagus missing');
    const guards=state.diagnostic.entities.filter(e=>[0xd2a,0xd2b].includes(e.kind)&&e.tomb?.anchor===sarc.id);
    assert(guards.length===8&&guards.every(e=>e.hp===(e.kind===0xd2a?8000:7000)),'Eight undamaged original worshippers required');
    const target=guards.filter(e=>e.kind===0xd2a).sort((a,b)=>Math.hypot(a.pos.x-state.view.position.x,a.pos.y-state.view.position.y)-Math.hypot(b.pos.x-state.view.position.x,b.pos.y-state.view.position.y))[0];assert(target,'Original male worshipper missing');
    assert(normal.every(s=>s.inspected.view.character.hp>300),'Party health insufficient for the bounded exposure');
    const config=JSON.parse(await readFile(join(out,'native/native.json'),'utf8')),browser=config.native.find(n=>n.actor===game.actor);assert(browser,'Owned lead browser missing');
    connection=await connect({...browser,url:config.origin+'/',helper:config.browserTools});
    const rect=await connection.eval('(()=>{const r=document.querySelector("ruffle-player").getBoundingClientRect();return{x:r.x,y:r.y,width:r.width,height:r.height};})()');assert(rect.width===800&&rect.height===600,'Native player must be unscaled');
    async function sample(){const frame=await game.inspect(),entity=frame.diagnostic.entities.find(e=>e.id===target.id);assert(entity,'Original worshipper disappeared');latest={time:frame.view.time_ms,stage:entity.stage};samples.push(frame);return{frame,entity};}
    let active=true;
    const action=game.act(Array.from({length:3},()=>group(4000,true,target.id)),{name:'tomb-worshipper-black-retreat',when:v=>Boolean(latest&&[1,2].includes(latest.stage)&&v.lifecycle.alive&&v.game_ms>=latest.time)}).finally(()=>active=false);
    const tracking=(async()=>{
      for(let i=0;active&&(await game.observe()).paused;i++){assert(i<250,'Native party did not unpause');await delay(20);}
      let previous;
      while(active){
        const {frame}=await sample();if(!active)break;
        const body=frame.view.objects.find(e=>e.id===target.id&&e.hp>0);assert(body,'Original worshipper left the native neighborhood');
        if(previous&&frame.view.time_ms>previous.time){
          const dt=(frame.view.time_ms-previous.time)/1000,vx=(body.position.x-previous.pos.x)/dt,vy=(body.position.y-previous.pos.y)/dt;
          const t=Math.min(0.35,Math.hypot(body.position.x-frame.view.position.x,body.position.y-frame.view.position.y)/10),dx=body.position.x+vx*t-frame.view.position.x,dy=body.position.y+vy*t-frame.view.position.y,a=-Math.PI/4;
          const x=50*(dx*Math.cos(a)+dy*Math.sin(a)),y=50*(-dx*Math.sin(a)+dy*Math.cos(a)),scale=Math.max(1,Math.hypot(x,y)/270),mouse={x:rect.x+300+x/scale,y:rect.y+325+y/scale};
          assert(Number.isFinite(mouse.x)&&Number.isFinite(mouse.y),'Nonfinite native aim');
          await connection.call('Input.dispatchMouseEvent',{type:'mouseMoved',buttons:1,...mouse});pointers.push({time:frame.view.time_ms,target:body.position,mouse});
        }
        previous={time:frame.view.time_ms,pos:body.position};await delay(80);
      }
    })();
    const settled=await Promise.allSettled([action,tracking]);for(const r of settled)if(r.status==='rejected')throw r.reason;
    await save('after-fire');
    for(let n=0;n<6;n++){
      const {entity}=await sample();if(entity.stage===3)break;
      const party=await save('healing-wait-'+n);assert(party.every(s=>s.observed.lifecycle.alive&&s.inspected.view.character.hp>300),'Party health abort before another exposure');
      assert(party[0].observed.game_ms<26000,'Worshipper transition exposure exhausted');
      await game.act([group(1000,party[2].inspected.view.character.mp>=100)]);
    }
    const {entity}=await sample();assert(entity.stage===3,'Original worshipper did not reach its yellow enlarged orbit');
    const party=await save('yellow-ready');assert(party.every(s=>s.observed.lifecycle.alive&&s.inspected.view.character.hp>300),'Party health abort before capture');
    await game.act([group(1000,party[2].inspected.view.character.mp>=100)],{name:'tomb-worshipper-yellow-orbit',when:v=>v.lifecycle.alive});
    await save('yellow');
    await game.act([peers.map(g=>({actor:g.actor,input:{keys:['Insert'],duration_ms:300}}))]);
    const escaped=await save('escaped'),stages=samples.map(s=>s.diagnostic.entities.find(e=>e.id===target.id));
    game.check('ordinary sword damage triggers the original priest retreat',stages.some(e=>e.stage===1&&e.hp<2400)&&escaped[0].inspected.diagnostic.character.shots>0);
    game.check('same priest naturally heals to full health then enlarges',stages.some(e=>e.stage===2&&e.hp===8000)&&stages.some(e=>e.stage===3&&e.size===120));
    game.check('prepared Sarcophagus survives without awakening bosses',snapshots.filter(s=>s.label!=='escaped').every(s=>s.actors[0].inspected.diagnostic.entities.some(e=>e.id===sarc.id&&e.hp>0)&&!s.actors[0].inspected.diagnostic.entities.some(e=>[0xd26,0xd27,0xd28].includes(e.kind))));
    game.check('same three living characters escape without kills or Tomb credit',escaped.every((s,n)=>s.observed.world==='Nexus'&&s.observed.lifecycle.alive&&s.observed.session.character===before[n].observed.session.character&&s.inspected.diagnostic.character.kills===0&&s.inspected.diagnostic.character.tombs===0));
  }finally{connection?.close();await writeFile(join(out,'worshipper-phases.json'),JSON.stringify({qualification:'Prepared component test, two legal max-stat Knights plus one Priest with ordinary selected250HealNova/4sArmored Tome. No NPC/HP/RNG edits, injected protection, support bots, refill or relocation. One sword fires under the native lease with recorded owned-page pointer tracking; input holds are not total exposure. Current reconstruction phases, not historical heal-rate/AI/full-dungeon acceptance.',snapshots,samples,pointers},null,2));}
}
