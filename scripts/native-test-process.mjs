import {spawn} from 'node:child_process';
import {readFileSync,openSync,closeSync,accessSync,writeFileSync,renameSync} from 'node:fs';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {setTimeout as delay} from 'node:timers/promises';

function vanished(pid,error) {
  if(!['ENOENT','ESRCH'].includes(error.code))throw error;
  try{accessSync('/proc/'+pid);}catch(proof){
    if(['ENOENT','ESRCH'].includes(proof.code))return null;
    throw new AggregateError([error,proof],'Cannot prove owned PID absent: '+pid,{cause:error});
  }
  throw Error('Owned PID exists without readable metadata: '+pid,{cause:error});
}
function readIdentity(pid,group=false) {
  if(!Number.isSafeInteger(pid)||pid<=0)throw Error('Invalid owned PID: '+pid);
  let fd;
  try{fd=openSync('/proc/'+pid,'r');}catch(error){return vanished(pid,error);}
  let primary;
  try{
    const path='/proc/self/fd/'+fd;let status,stat;
    try{status=readFileSync(path+'/status','utf8');}catch(error){return vanished(pid,error);}
    const credentials=/^Uid:[ \t]+(\d+)[ \t]+(\d+)[ \t]+(\d+)[ \t]+(\d+)[ \t]*$/m.exec(status),uid=process.getuid();
    if(!credentials||!credentials.slice(1).every(value=>Number.isSafeInteger(Number(value))&&Number(value)===uid))throw Error('Owned PID kernel UID mismatch or invalid credentials: '+pid+' expected='+uid+' observed='+JSON.stringify(credentials?.slice(1)));
    if(/^State:[ \t]+[ZXx](?:[ \t]|$)/m.test(status))return null;
    try{stat=readFileSync(path+'/stat','utf8');}catch(error){return vanished(pid,error);}
    const fields=stat.slice(stat.lastIndexOf(')')+2).trim().split(/\s+/);
    if(!stat.startsWith(pid+' (')||!/^\d+$/.test(fields[19]))throw Error('Invalid owned PID stat/start metadata: '+pid);
    if(['Z','X','x'].includes(fields[0]))return null;
    if(group&&(fields[2]!==String(pid)||fields[3]!==String(pid)))throw Error('Owned PID process group/session mismatch: '+pid);
    return {pid,uid,start:fields[19]};
  }catch(error){primary=error;throw error;}finally{
    try{closeSync(fd);}catch(error){
      if(primary)throw new AggregateError([primary,error],'Owned proc descriptor close failed after metadata failure: '+pid,{cause:primary});
      throw error;
    }
  }
}
export function identity(pid) {return readIdentity(pid);}
function checkedProcess(row,group=false) {
  if(!row||row.uid!==process.getuid()||typeof row.start!=='string'||!/^\d+$/.test(row.start))throw Error('Missing or invalid owned creation token for PID: '+row?.pid);
  const live=readIdentity(row.pid,group);
  if(live&&(live.uid!==row.uid||live.start!==row.start))throw Error('Owned PID creation token mismatch: '+row.pid+' expected='+row.start+' observed='+live.start);
  return live;
}
export function sameProcess(row) {return checkedProcess(row)?true:null;}
function signalProcess(row,signal,group) {
  if(!checkedProcess(row,group))return;
  try{process.kill(group?-row.pid:row.pid,signal);}catch(error){
    if(error.code!=='ESRCH')throw error;
    try{if(checkedProcess(row,group))throw Error('Owned PID still live after signal ESRCH: '+row.pid);}
    catch(proof){throw new AggregateError([error,proof],'Owned signal ESRCH lacks absence/death proof: '+row.pid,{cause:error});}
  }
}
export function registry(path,value) {writeFileSync(path+'.tmp',JSON.stringify(value),{mode:0o600});renameSync(path+'.tmp',path);}
export function startChild(command,args,options,signal) {
  signal.throwIfAborted();const child=spawn(command,args,{...options,detached:true});
  const row={child,done:false,identity:null};
  row.closed=new Promise(resolve=>child.once('close',(code,killed)=>{row.done=true;resolve({pid:child.pid,code,signal:killed});}));
  // Promise continuation lets the caller retain the creation before metadata can fail.
  row.started=new Promise((resolve,reject)=>{child.once('spawn',resolve);child.once('error',reject);}).then(()=>{
    if(row.done||child.exitCode!==null||child.signalCode!==null)throw Error('Owned child exited before bootstrap identity: '+child.pid);
    row.identity=identity(child.pid);
    if(!row.identity)throw Error('Owned child has no live bootstrap identity: '+child.pid);
  });row.started.catch(()=>{});
  return row;
}
export async function stopChild(row,grace=3000) {
  await row.started;
  if(row.done)return await row.closed;
  for(const [name,ms]of [['SIGINT',grace],['SIGTERM',2000],['SIGKILL',5000]]){
    if(row.done)return await row.closed;
    signalProcess(row.identity,name,true);
    const stopped=await Promise.race([row.closed.then(value=>({value})),delay(ms,undefined,{ref:false}).then(()=>null)]);
    if(stopped){row.done=true;return stopped.value;}
  }
  throw Error('Owned child did not close after SIGKILL: '+row.child.pid);
}
export async function guardRun(directory) {
  const path=join(directory,'children.json'),boot=readFileSync('/proc/sys/kernel/random/boot_id','utf8');let state,primary,reason;
  try{while(true){
    const next=JSON.parse(readFileSync(path,'utf8'));
    if(next.boot!==boot||next.parent.uid!==process.getuid())throw Error('Guardian owner/boot mismatch');
    state=next;if(state.done)return;
    if(!sameProcess(state.parent)){reason='Runner process disappeared';break;}
    if(Date.now()>=state.deadline){reason='Absolute run deadline reached';break;}
    await delay(250);
  }}catch(error){primary=error;reason='Guardian ownership failure: '+String(error);}
  if(!state)throw primary;
  const children=state.children,errors=primary?[primary]:[],failed=new Set();
  try{
    registry(join(directory,'guardian.json'),{reason,children});
    const resultPath=join(directory,'result.json'),result=JSON.parse(readFileSync(resultPath,'utf8'));
    if(result.status==='running'){result.status='failed';result.failure={phase:'guardian',error:reason};writeFileSync(resultPath+'.guardian',JSON.stringify(result,null,2));renameSync(resultPath+'.guardian',resultPath);}
  }catch(error){errors.push(error);}
  const remaining=()=>children.filter(row=>{
    if(failed.has(row))return true;
    try{return Boolean(sameProcess(row));}catch(error){failed.add(row);errors.push(error);return true;}
  });
  for(const [signal,ms]of [['SIGINT',1000],['SIGTERM',1000],['SIGKILL',2000]]){
    for(const row of [...children].reverse())if(!failed.has(row))try{signalProcess(row,signal,true);}catch(error){failed.add(row);errors.push(error);}
    await delay(ms);if(!remaining().length)break;
  }
  try{signalProcess(state.parent,'SIGKILL',false);}catch(error){errors.push(error);}
  const live=remaining();
  try{registry(join(directory,'guardian.json'),{reason,remaining:live,children,errors:errors.map(String)});}catch(error){errors.push(error);}
  if(live.length)errors.push(Error('Guardian could not prove owned children retired: '+live.map(row=>row.pid).join(',')));
  if(errors.length)throw new AggregateError(errors,'Guardian cleanup failed',{cause:primary??errors[0]});
}
if(process.argv[1]===fileURLToPath(import.meta.url))await guardRun(process.argv[2]);
