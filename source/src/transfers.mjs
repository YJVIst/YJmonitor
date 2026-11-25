// These identifiers authorize file checkpoints only; they never restore an SSH shell.
export function findTransferContext(node,claimed=new Set()){
 try{const candidates=[];for(let i=0;i<localStorage.length;i++){const key=localStorage.key(i);if(!/^yuji-transfers:[a-f0-9]{64}$/.test(key||''))continue;const id=key.slice(15);if(claimed.has(id))continue;const saved=JSON.parse(localStorage.getItem(key)||'null');if(saved?.node===node&&Date.now()-saved.at<1800000&&Array.isArray(saved.jobs)&&saved.jobs.length)candidates.push({id,at:saved.at});}return candidates.sort((a,b)=>b.at-a.at)[0]?.id||'';}catch{return '';}
}
import {exportDownload} from './download-stream.mjs';
const successDisplayMs=3000;
const MAX=2*1024**3,CHUNK=131072;
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const el=(tag,text)=>{const n=document.createElement(tag);if(text!==undefined)n.textContent=text;return n;};
function engine(){
 const w=new Worker(new URL('__TRANSFER_WORKER__',location.origin),{type:'module'}),jobs=new Map();let sequence=0;
 w.onmessage=({data:v})=>{const p=jobs.get(v.request);if(!p)return;jobs.delete(v.request);clearTimeout(p.timer);v.ok?p.resolve(v.data):p.reject(Error(v.error));};
 w.onerror=()=>{for(const p of jobs.values()){clearTimeout(p.timer);p.reject(Error('文件处理线程异常'));}jobs.clear();};
 return {cancelExport:()=>w.postMessage({action:'cancel-export'}),run:(action,value={},transfer=[])=>new Promise((resolve,reject)=>{const request=++sequence,timer=setTimeout(()=>{jobs.delete(request);reject(Error('文件处理超时'));},action==='stream'?1800000:180000);jobs.set(request,{resolve,reject,timer});w.postMessage({request,action,...value},transfer);}),destroy:()=>{w.terminate();for(const p of jobs.values()){clearTimeout(p.timer);p.reject(Error('文件处理已结束'));}jobs.clear();}};
}
export function createTransfers({holder,request,note,refresh}){
 let active=false,session='',node='',running=null,generation=0,worker=null;const jobs=[],box=el('section'),title=el('strong','传输队列'),list=el('div'),choose=el('input');box.className='file-transfer-queue';choose.type='file';choose.hidden=true;box.append(title,list,choose);holder.append(box);let selecting=null;
 const key=()=>session?'yuji-transfers:'+session:'';
 function persist(){if(!key())return;try{const value=jobs.filter(j=>!['done','cancelled'].includes(j.state)).map(({file,...j})=>({...j,state:'paused',reason:''}));if(value.length)localStorage.setItem(key(),JSON.stringify({at:Date.now(),node,jobs:value}));else localStorage.removeItem(key());}catch{}}
 function rowButton(label,fn){const b=el('button',label);b.type='button';b.onclick=fn;return b;}
 const closeTimers=new Map();
 function dismiss(job){
  clearTimeout(closeTimers.get(job));closeTimers.delete(job);
  const index=jobs.indexOf(job);
  if(index<0)return;
  jobs.splice(index,1);draw();
 }
 function clearTimers(){
  for(const timer of closeTimers.values())clearTimeout(timer);
  closeTimers.clear();
 }
 function showSuccess(job,message){
  if(!jobs.includes(job))return;
  job.state='done';job.reason=message;job.file=null;
  persist();draw();
  closeTimers.set(job,setTimeout(()=>dismiss(job),successDisplayMs));
 }
 function directoryCreated(parent,name){
  const job={name,path:(parent==='/'?'':parent)+'/'+name,directory:true,state:'done'};
  jobs.push(job);showSuccess(job,'目录已创建');
 }
 function draw(){box.hidden=!jobs.length;list.replaceChildren();for(const j of jobs){const row=el('div'),label=el('span',(j.directory?'':j.upload?'↑ ':'↓ ')+j.name),info=el('small',j.reason||({waiting:'等待中',active:'传输中',paused:'已暂停',done:'已完成',cancelled:'已取消',error:'未完成'}[j.state]||'')+' · '+Math.round(j.size?j.offset/j.size*100:0)+'%');row.className='file-transfer-job';label.title=j.path;row.append(label,info);
 if(j.state==='active')row.append(rowButton('暂停',()=>{j.pause=true;worker?.cancelExport();}));
 if(['error','paused'].includes(j.state))row.append(rowButton(j.upload&&!j.file?'选择原文件续传':'继续',()=>{if(j.upload&&!j.file){selecting=j;choose.click();}else{j.state='waiting';j.reason='';j.pause=false;draw();pump();}}));
 if(!['done','cancelled'].includes(j.state))row.append(rowButton('取消',()=>cancel(j)));else row.append(rowButton('关闭',()=>dismiss(j)));list.append(row);}}
 choose.onchange=()=>{const f=choose.files[0],j=selecting;choose.value='';selecting=null;if(!f||!j)return;if(f.name!==j.name||f.size!==j.size){note('请选择名称和大小一致的原文件，续传前还会核验内容。',true);return;}j.file=f;j.state='waiting';j.reason='';j.pause=false;draw();pump();};
 async function cancel(j){j.cancel=true;if(running===j){worker?.cancelExport();return;}try{if(j.id&&active)await request('transfer_cancel',j.path,{transfer:j.id});if(j.id){const e=engine();await e.run('remove',{id:j.id}).catch(()=>{});e.destroy();}j.state='cancelled';j.file=null;persist();draw();}catch(e){j.reason=e.message;draw();}}
 async function add(file,path,creating=false){if(!active)return null;if(jobs.filter(j=>!['done','cancelled'].includes(j.state)).length>=20){note('队列最多保留 20 个任务',true);return null;}if(file&&file.size>MAX){note('单个文件最大 2 GiB',true);return null;}
 if(!file&&(!navigator.storage?.getDirectory||!navigator.serviceWorker)){note('此浏览器不支持低内存下载，请使用支持 OPFS 的新版浏览器。',true);return null;}
 const job={key:crypto.randomUUID(),id:'',upload:!!file,creating,file,name:file?.name||path.split('/').pop(),path,parent:path,size:file?.size||0,offset:0,state:'waiting',reason:'',pause:false,cancel:false};jobs.push(job);draw();persist();pump();return job;
 }
 async function pump(){if(!active||running)return;const j=jobs.find(j=>j.state==='waiting');if(!j)return;running=j;j.state='active';j.reason='';j.pause=false;const epoch=generation;const e=engine();worker=e;
 try{
  if(j.exportReady){j.reason='正在核对下载缓存';draw();const v=await e.run('init',{id:j.id,upload:false,offset:j.size});if(v.sha256!==j.sha256)throw Error('下载缓存校验不一致，请重新下载');await e.run('finish');j.reason='正在保存到浏览器';draw();await exportDownload(e,j.name,j.size);await request('transfer_cancel',j.path,{transfer:j.id}).catch(()=>{});j.state='done';j.reason='';await e.run('remove',{id:j.id});persist();return;}
  let r;if(j.id){const saved=await request('transfer_list','/');r=saved.transfers.find(v=>v.transfer===j.id);if(!r)throw Error('远端续传记录已结束，请移除此任务后重新开始');j.offset=r.offset;j.size=r.size;
   j.reason='正在核对已传输内容';draw();const local=await e.run('init',{id:j.id,upload:j.upload,file:j.file,offset:j.offset});if(local.sha256!==r.sha256)throw Error('已传输内容校验不一致，已阻止续传');
   r=await request('transfer_resume',j.path,{transfer:j.id,revision:local.sha256});
  }else{r=await request(j.upload?'upload_start':'download_start',j.parent,j.upload?{target:j.name,size:j.size}:{});j.id=r.transfer;j.path=r.path;j.size=r.size;j.offset=0;
   if(!j.upload){const storage=await navigator.storage.estimate();if(storage.quota&&storage.quota-(storage.usage||0)<j.size+16*1024**2)throw Error('手机可用缓存空间不足，请释放空间后继续');}
   await e.run('init',{id:j.id,upload:j.upload,file:j.file,offset:0});
  }
  j.reason='';persist();draw();let sum=r.sha256,done=false;
  while(j.upload?j.offset<j.size:!done){
   if(j.cancel)throw Error('已取消');if(j.pause||!active||epoch!==generation)throw Error('已暂停');
   const at=performance.now();let result;
   if(j.upload){const block=await e.run('read',{offset:j.offset,size:CHUNK});result=await request('upload_chunk',j.path,{transfer:j.id,offset:j.offset,content:block.content});if(result.sha256!==block.sha256)throw Error('上传校验不一致');j.offset=result.offset;sum=block.sha256;}
   else{result=await request('download_chunk',j.path,{transfer:j.id,offset:j.offset});const block=await e.run('write',{offset:j.offset,content:result.content});if(block.sha256!==result.sha256)throw Error('下载校验不一致');j.offset=block.offset;sum=block.sha256;done=result.done;}
   persist();draw();await pause(Math.max(0,125-(performance.now()-at)));
  }
  if(j.cancel||j.pause||!active||epoch!==generation)throw Error(j.cancel?'已取消':'已暂停');
  if(j.upload){if(!sum){const empty=await e.run('init',{id:j.id,upload:true,file:j.file,offset:j.offset});sum=empty.sha256;}await request('upload_finish',j.path,{transfer:j.id,offset:j.offset,revision:sum});if(jobs.includes(j)){showSuccess(j,j.creating?'文件已创建':'上传完成');if(active&&epoch===generation)await refresh();}}
  else{const result=await e.run('finish');if(result.sha256!==sum)throw Error('最终校验失败');j.exportReady=true;j.sha256=sum;persist();await request('download_finish',j.path,{transfer:j.id,offset:j.offset,revision:sum});j.reason='正在保存到浏览器';draw();await exportDownload(e,j.name,j.size);await e.run('remove',{id:j.id});note('下载已交给浏览器保存 · '+j.name);}
  if(!j.upload){j.state='done';j.reason='';j.file=null;persist();}
 }catch(err){if(j.cancel){if(active&&j.id)await request('transfer_cancel',j.path,{transfer:j.id}).catch(()=>{});await e.run('remove',{id:j.id}).catch(()=>{});j.state='cancelled';j.file=null;j.reason='';}else{j.state=j.pause||!active||epoch!==generation?'paused':'error';j.reason=j.state==='error'?err.message:'';if(active&&j.id)await request('transfer_pause',j.path,{transfer:j.id}).catch(()=>{});}persist();}
 finally{await e.run('close').catch(()=>{});e.destroy();if(worker===e)worker=null;if(running===j)running=null;draw();if(active)pump();}
 }
 function discardLocal(remove=false){clearTimers();active=false;generation++;const ids=jobs.map(j=>j.id).filter(Boolean);for(const j of jobs){j.cancel=true;j.file=null;}jobs.length=0;try{localStorage.removeItem(key());}catch{}const old=worker;old?.cancelExport();Promise.resolve(old?.run('close')).catch(()=>{}).then(async()=>{const cleanup=engine();for(const id of ids)await cleanup.run('remove',{id}).catch(()=>{});cleanup.destroy();});if(remove)box.remove();else draw();}
 return {add,directoryCreated,isBusy:()=>jobs.some(j=>!['done','cancelled'].includes(j.state)),active:async(context)=>{const previous=session;session=context.session||'';node=context.node||'';active=true;if(previous!==session){clearTimers();jobs.length=0;try{const saved=JSON.parse(localStorage.getItem(key())||'null');if(saved&&Date.now()-saved.at<1800000&&Array.isArray(saved.jobs))for(const j of saved.jobs.slice(0,20))if(typeof j.id==='string'&&/^[a-f0-9]{64}$/.test(j.id)&&typeof j.path==='string')jobs.push({...j,file:null,state:'paused',cancel:false,pause:false});}catch{}}
 try{const r=await request('transfer_list','/');for(const t of r.transfers)if(!jobs.some(j=>j.id===t.transfer))jobs.push({key:crypto.randomUUID(),id:t.transfer,upload:t.upload,file:null,name:t.name,path:t.path,parent:t.path,size:t.size,offset:t.offset,state:'paused',reason:'',pause:false,cancel:false});}catch{}draw();pump();},
 reset:()=>discardLocal(),suspend:()=>{active=false;generation++;if(running)running.pause=true;worker?.cancelExport();persist();draw();},discard:async()=>{for(const j of jobs)await cancel(j);},destroy:({keep=false}={})=>{clearTimers();active=false;generation++;if(!keep){for(const j of jobs)j.cancel=true;try{localStorage.removeItem(key());}catch{}}else persist();const prior=worker;prior?.cancelExport();const ids=jobs.map(j=>j.id).filter(Boolean);Promise.resolve(prior?.run('close')).catch(()=>{}).then(async()=>{if(!keep){const cleanup=engine();for(const id of ids)await cleanup.run('remove',{id}).catch(()=>{});cleanup.destroy();}});jobs.length=0;box.remove();},get jobs(){return jobs;}};
}

