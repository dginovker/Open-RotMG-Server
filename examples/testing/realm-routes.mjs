import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {inflateSync} from 'node:zlib';

const layout=process.env.REALM_LAYOUT,firstCoast=Number(process.env.REALM_ARRIVAL);
assert(['02','03'].includes(layout),'Set REALM_LAYOUT to 02 or 03');
assert(/^[0-3]$/.test(process.env.REALM_ARRIVAL),'Set REALM_ARRIVAL to 0, 1, 2 or 3');
const spec={class:0x307,level:20,seed:785,world:-3,position:'spawn',equipment:[0xc02,-1,-1,-1],backpack:Array(8).fill(-1),stats:[700,252,50,25,50,50,40,50]};
let world,worldId;
const arrival=i=>({x:world.spawn_points[i][0],y:world.spawn_points[i][1]});
export async function prepare(api){
  const worlds=await Promise.all([-3].map(async id=>({id,info:await api({op:'world_info',world:id})})));
  await writeFile(join(process.env.QA_RESULTS_DIR,'startup-realms.json'),JSON.stringify(worlds));
  const selected=worlds.find(w=>w.info.realm_layout===`classic-world-${layout}-reconstruction`);
  assert(selected,`World ${layout} is absent from this startup's random Realm selection`);
  world=selected.info;worldId=selected.id;
  assert.equal(world.spawn_points.length,12);
  return {...spec,world:worldId,position:arrival(firstCoast)};
}
export async function run(firstClient){
  const root=process.env.QA_RESULTS_DIR,config=JSON.parse(await readFile(join(root,'native/native.json'),'utf8'));
  const token=(await readFile(join(config.state,'control.token'),'utf8')).trim();let serial=0;
  const api=async command=>{
    const response=await fetch(config.origin+'/dev/testing',{method:'POST',headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},body:JSON.stringify({id:'route-'+(++serial),command}),signal:AbortSignal.timeout(10000)});
    const result=await response.json();assert(response.ok&&result.status!=='failed',JSON.stringify(result));return result;
  };
  const peer=await firstClient.join({...spec,seed:786,world:worldId,position:arrival(firstCoast)});
  const opposite=(firstCoast+2)%4;
  const remote=await firstClient.join({...spec,seed:787,world:worldId,position:arrival(opposite)});
  const clients=[firstClient,peer,remote],first=await firstClient.observe();
  const response=await fetch(config.origin+'/dev/map?player='+first.session.player,{headers:{Authorization:'Bearer '+token},signal:AbortSignal.timeout(30000)});
  assert(response.ok);const map=await response.json();
  const dictionary=JSON.parse(await readFile(`data/realms/world${layout}/cells.dictionary.json`,'utf8'));
  const indices=inflateSync(await readFile(`data/realms/world${layout}/cells.indices.zlib`));
  const width=map.width,total=width*map.height,blocked=new Uint8Array(total);
  for(const [x,y] of map.blocked)blocked[y*width+x]=1;
  const ground=i=>dictionary[indices.readUInt16LE(i*2)].ground;
  const routes=[];let protectionUntil=0;
  const save=()=>writeFile(join(root,'routes.json'),JSON.stringify({world,worldId,prepared:'level 20, explicit legal stats, Doom Bow, full-map routing, invincibility; no teleports',routes},null,2));
  for(const [game,coast] of [[firstClient,firstCoast],[remote,opposite]]){
    const start=arrival(coast);
    const initial=await game.observe();
    game.check(`coast ${coast}: registered arrival in the shared Realm`,initial.session.world===worldId&&Math.hypot(initial.position.x-start.x,initial.position.y-start.y)<.1);
    const parent=new Int32Array(total).fill(-1),queue=new Int32Array(total),origin=Math.floor(start.y)*width+Math.floor(start.x);
    parent[origin]=origin;queue[0]=origin;let head=0,tail=1,target;
    // These are current reconstructions, not recovered geography: https://github.com/OpenRotMG/Open-RotMG-Server/issues/785#issuecomment-6454565800
    while(head<tail){
      const i=queue[head++],x=i%width,y=Math.floor(i/width);
      if(ground(i)===0xd0&&Math.hypot(x+.5-start.x,y+.5-start.y)>=32){target=i;break;}
      assert(head<400000,'No bounded coastal-to-road route');
      for(const n of [i-width,i-1,i+1,i+width])if(n>=0&&n<total&&Math.abs(n%width-x)<=1&&parent[n]===-1&&!blocked[n]&&![0x72,0x73,0xbc,0xf4,0xc].includes(ground(n))){parent[n]=i;queue[tail++]=n;}
    }
    assert.notEqual(target,undefined,'No dry coastal-to-road route');
    const path=[];for(let i=target;i!==origin;i=parent[i])path.push(i);path.push(origin);path.reverse();
    const points=path.filter((p,i)=>i===0||i===path.length-1||p-path[i-1]!==path[i+1]-p).map(i=>({x:i%width+.5,y:Math.floor(i/width)+.5}));
    const route={coast,start,target:points.at(-1),cells:path.length-1,points,observations:[]};routes.push(route);await save();
    let now=await game.observe();
    if(now.game_ms>=protectionUntil){for(const client of clients)await api({op:'cheat',actor:client.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});protectionUntil=now.game_ms+60000;}
    await game.act({keys:[],duration_ms:500},{name:`world-${layout}-arrival-${coast}`,when:()=>true});
    const a=await game.observe(),b=await peer.observe();
    game.check(`coast ${coast}: clients share the Realm`,a.session.world===worldId&&b.session.world===worldId);
    if(game===firstClient)game.check(`coast ${coast}: mutual observer identity`,a.nearby.some(o=>o.id===b.session.player)&&b.nearby.some(o=>o.id===a.session.player));
    let stalled=0;
    for(const point of points.slice(1))for(let step=0;step<80;step++){
      const before=await game.observe(),dx=point.x-before.position.x,dy=point.y-before.position.y;
      if(Math.hypot(dx,dy)<.45)break;
      assert(step<79,'Waypoint input budget exhausted');
      if(before.game_ms>=protectionUntil){for(const client of clients)await api({op:'cheat',actor:client.actor,change:{kind:'protection',condition:'invincible',duration_ms:60000}});protectionUntil=before.game_ms+60000;}
      const horizontal=Math.abs(dx)>Math.abs(dy),distance=horizontal?dx:dy;
      const keys=horizontal?(dx>0?['d','s']:['a','w']):(dy>0?['a','s']:['d','w']);
      await game.act({keys,duration_ms:Math.max(100,Math.min(1600,Math.round(Math.abs(distance)*900/7.734)))});
      const after=await game.observe();route.observations.push({position:after.position,game_ms:after.game_ms,hp:after.character.hp,nearby:after.nearby});await save();
      stalled=Math.hypot(after.position.x-before.position.x,after.position.y-before.position.y)>.02?0:stalled+1;
      assert(stalled<3,`Three stationary inputs at coast ${coast}: ${JSON.stringify({point,before:before.position,after:after.position})}`);
      assert(after.lifecycle.alive,'Prepared route walker died');
      if(game===firstClient&&route.observations.length===1){
        const observed=(await peer.inspect()).view.objects.find(o=>o.id===after.session.player);
        game.check(`coast ${coast}: observer receives native departure`,!!observed&&Math.hypot(observed.position.x-after.position.x,observed.position.y-after.position.y)<.2);
      }
    }
    await game.act({keys:[],duration_ms:500},{name:`world-${layout}-road-${coast}`,when:()=>true});
    const end=await game.inspect(),observer=await peer.observe();route.end=end;route.observer=observer;await save();
    game.check(`coast ${coast}: native input reaches inland road`,Math.hypot(end.view.position.x-route.target.x,end.view.position.y-route.target.y)<.5&&Math.hypot(end.view.position.x-start.x,end.view.position.y-start.y)>=31);
    const tile=end.view.tiles.find(t=>t.x===Math.floor(end.view.position.x)&&t.y===Math.floor(end.view.position.y));
    game.check(`coast ${coast}: actual mapped road underfoot`,tile?.ground===0xd0);
    for(const client of clients)game.check(`coast ${coast}: prepared character ${client.actor} survives`,(await client.observe()).lifecycle.alive);
    const identity=await api({op:'world_info',world:worldId});game.check(`coast ${coast}: shared Realm layout persists`,identity.realm_layout===world.realm_layout&&JSON.stringify(identity.spawn_points)===JSON.stringify(world.spawn_points));
  }
}
