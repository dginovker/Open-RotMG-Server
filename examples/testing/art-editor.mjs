import assert from 'node:assert/strict';
import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {inflateSync} from 'node:zlib';
import {setTimeout as delay} from 'node:timers/promises';
import {connect} from '../../scripts/record-demo.mjs';

export const fixture={class:0x30e,level:1,seed:934,world:-2,position:{x:74.5,y:78.5},equipment:[0xa97,0xa2e,-1,-1],backpack:Array(8).fill(-1),stats:null};
const out=process.env.QA_RESULTS_DIR,exec=promisify(execFile),traffic=[],pending=new Map();
let c,networkError,owner,menus=0;
export async function beforeNavigate({browser}) {
  c=await connect({...browser,url:'about:blank',helper:join(process.env.PI_BROWSER_TOOLS,'direct-cdp.mjs'),onEvent:e=>{
    if(e.method==='Network.requestWillBeSent'&&/\/legacy-api\/(picture\/|char\/list)/.test(e.params.request.url))pending.set(e.params.requestId,new URL(e.params.request.url).pathname);
    if(e.method==='Network.loadingFinished'&&pending.has(e.params.requestId)) {
      const path=pending.get(e.params.requestId);pending.delete(e.params.requestId);
      if(path.endsWith('/char/list')){menus++;return;}
      c.call('Network.getResponseBody',{requestId:e.params.requestId}).then(r=>traffic.push({path,body:r.body,base64:r.base64Encoded})).catch(error=>networkError=error);
    }
  }});
  await c.call('Network.enable');
  await c.call('Emulation.setFocusEmulationEnabled',{enabled:true});
}
async function until(predicate,label) {
  for(let i=0;i<300;i++){if(networkError)throw networkError;if(await predicate())return;await delay(100);}
  throw Error('Native art editor did not reach '+label);
}
async function click(x,y) {
  const r=await c.eval('(()=>{const r=document.querySelector("ruffle-player").getBoundingClientRect();return{x:r.x,y:r.y,width:r.width,height:r.height};})()');
  assert.equal(r.width,800);assert.equal(r.height,600);
  const p={x:r.x+x,y:r.y+y,button:'left',clickCount:1};
  await c.call('Input.dispatchMouseEvent',{type:'mouseMoved',x:p.x,y:p.y});
  try{await c.call('Input.dispatchMouseEvent',{type:'mousePressed',buttons:1,...p});}
  finally{await c.call('Input.dispatchMouseEvent',{type:'mouseReleased',buttons:0,...p});}
  await delay(150);
}
async function type(x,y,text) {
  await click(x,y);
  for(const kind of ['keyDown','keyUp'])await c.call('Input.dispatchKeyEvent',{type:kind,key:'End',code:'End',windowsVirtualKeyCode:35});
  for(let i=0;i<32;i++)for(const kind of ['keyDown','keyUp'])await c.call('Input.dispatchKeyEvent',{type:kind,key:'Backspace',code:'Backspace',windowsVirtualKeyCode:8});
  for(const letter of text)await c.call('Input.dispatchKeyEvent',{type:'char',text:letter});
}
async function shot(name) {
  await writeFile(join(out,name+'.png'),Buffer.from((await c.call('Page.captureScreenshot',{format:'png'})).data,'base64'));
}
async function rows() {
  return JSON.parse((await exec('python3',['-c','import sqlite3,json,sys; c=sqlite3.connect(sys.argv[1]); print(json.dumps(c.execute("SELECT id,account,name,data_type,tags,hex(png) FROM pictures ORDER BY id").fetchall()))',join(out,'native/state/game.db')])).stdout);
}
function image(row) {
  const png=Buffer.from(row[5],'hex'),width=png.readUInt32BE(16),height=png.readUInt32BE(20),compressed=[];
  for(let n=8;n<png.length;){const len=png.readUInt32BE(n);if(png.toString('ascii',n+4,n+8)==='IDAT')compressed.push(png.subarray(n+8,n+8+len));n+=len+12;}
  const pixels=inflateSync(Buffer.concat(compressed));
  assert.equal(pixels.length,(width*4+1)*height);
  for(let y=0;y<height;y++)assert.equal(pixels[y*(width*4+1)],0,'Selected PNG encoder changed its row filter');
  return {width,height,pixel:(x,y)=>[...pixels.subarray(y*(width*4+1)+1+x*4,y*(width*4+1)+5+x*4)]};
}
const replies=op=>traffic.filter(t=>t.path==='/legacy-api/picture/'+op);
async function save(name,kind=2,tags='qa,editor',fixedType=false) {
  await click(45,294);
  await type(380,175,name);await type(380,307,tags);
  if(!fixedType){await click(430,245);await click(430,245+kind*44);}
  const count=replies('save').length;
  await click(520,456);
  await until(()=>replies('save').length===count+1,'committed save '+name);
  const response=replies('save').at(-1).body;
  assert.match(response,/^<Success><Id>\d+<\/Id><\/Success>$/);
  const row=(await rows()).find(r=>r[0]===Number(response.match(/<Id>(\d+)/)[1]));
  assert(row);assert.equal(row[1],owner);assert.equal(row[2],name);assert.equal(row[3],kind);assert.equal(row[4],tags);
  return row;
}
async function loadFirst() {
  const count=replies('get').length;
  await click(45,263);
  await until(()=>replies('get').length>count,'Mine gallery thumbnail');await delay(500);
  await click(140,180);await delay(500);
}
async function navigate(url) {
  const count=menus;await c.call('Page.navigate',{url});
  await until(()=>menus>count,'account menu');
  await until(()=>c.eval('document.querySelector("ruffle-player")?.ruffle().isPlaying===true'),'playing client');
  await c.eval('(()=>{const m=document.querySelector("ruffle-player").shadowRoot.querySelector("#hardware-acceleration-modal");if(m&&!m.classList.contains("hidden"))m.querySelector(".close-modal").click();})()');
  await delay(1500);
}
export async function enter({account}) {
  owner=account;const origin=await c.eval('location.origin');
  await navigate(origin+'/draw.html');await shot('editor-before');
  await click(45,294);const count=replies('save').length;await click(520,456);
  assert.equal(replies('save').length,count,'Empty native name reached the server');await shot('empty-name-refused');
  await type(380,175,'Editor934');await click(520,456);
  assert.equal(replies('save').length,count,'Missing native type reached the server');await shot('missing-type-refused');
  await click(380,456);
  await click(55,490);await click(132,122);
  let row=await save('Editor934');const id=row[0];
  assert.deepEqual(image(row).pixel(0,0),[0,0,0,255]);
  await click(45,83);await click(132,122);row=await save('Editor934');assert.equal(row[0],id);assert.equal(image(row).pixel(0,0)[3],0);
  await click(45,143);row=await save('Editor934');assert.deepEqual(image(row).pixel(0,0),[0,0,0,255]);
  await click(45,173);row=await save('Editor934');assert.equal(image(row).pixel(0,0)[3],0);
  await click(45,143);await click(83,490);await click(45,113);await click(132,122);await click(45,54);await click(177,122);
  row=await save('Editor934');assert.deepEqual(image(row).pixel(1,0),[0,0,0,255]);
  await click(45,203);row=await save('Editor934');assert.equal(image(row).pixel(0,0)[3],0);assert.equal(image(row).pixel(1,0)[3],0);
  await click(45,143);row=await save('Editor934');assert.deepEqual(image(row).pixel(1,0),[0,0,0,255]);
  const persisted=row[5];await shot('tools-roundtrip');
  await navigate(origin+'/draw.html');await loadFirst();
  row=await save('Editor934');assert.equal(row[0],id);assert.equal(row[5],persisted,'Reload changed the saved bitmap');await shot('reconnected-art');
  await delay(61000);
  const sizes=[[8,8],[16,8],[16,16]];
  for(const [mode,dimensions,kind] of [[0,sizes,2],[1,sizes,1],[2,[[4,4],[5,5],[9,9],[10,10]],5]]) {
    await navigate(origin+'/draw.html');
    await click(350,45);await click(350,45+mode*26);
    for(let i=0;i<dimensions.length;i++) {
      const [w,h]=dimensions[i];await click(530,45);await click(530,45+i*26);
      const paint=async(x,frameWidth=w)=>{const cell=Math.min(Math.floor(360/frameWidth),Math.floor(360/h));await click(110+(360-cell*frameWidth)/2+cell*(x+.5),100+(360-cell*h)/2+cell*.5);};
      await click(55,490);await paint(0);
      if(mode===1) {
        for(const [tab,x] of [[1,210],[2,263],[3,321],[4,387]]){await click(x,82);await paint(tab===4?2*w-1:tab,tab===4?2*w:w);}
      }
      row=await save('Size'+mode+' '+w+' '+h,kind,'qa,size',mode!==0);
      const png=image(row);assert.equal(png.width,mode===1?7*w:w);assert.equal(png.height,h);assert.deepEqual(png.pixel(0,0),[0,0,0,255]);
      if(mode===1) {
        // The selected editor packs five states into seven widths, not seven frames: 123.5.1 §_-qD§/§_-XK§, https://openrotmg.org/restoration/reference-data/client-123.5.1.swf
        for(const x of [w+1,2*w+2,4*w+3,7*w-1])assert.deepEqual(png.pixel(x,0),[0,0,0,255]);
        assert.equal(png.pixel(3*w,0)[3],0);
      }
      await shot('size-'+mode+'-'+w+'-'+h);
    }
  }
  await writeFile(join(out,'art-evidence.json'),JSON.stringify({owner,id,traffic,pictures:await rows()},null,2));
  await navigate(origin);await click(400,550);await click(400,550);
}
export async function run(game) {
  game.check('native tools, refusals, overwrite, reconnect and all selected sizes/animation states preserve exact PNGs',(await rows()).length===11);
  c.close();
}
