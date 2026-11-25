import {compactBytes,workspaceWidth,workspaceHeight} from './terminal-monitor.mjs';
import {createTransfers} from './transfers.mjs';
const defaultDirectory='/root';
export const isInspection=action=>['processes','services','service_logs','system'].includes(action);
export function createFileBrowser({dialog,scope,send,onResize,onTerminal,onCommands=()=>{}}){
 const $=s=>scope.querySelector(s),sidebar=$('#file-sidebar'),tree=$('#file-tree'),pathInput=$('#file-path'),editor=$('#file-editor'),input=$('#file-content'),status=$('#file-editor-status');
 const pending=new Map(),cache=new Map();let active=false,filesAvailable=false,sequence=0,generation=0,root=defaultDirectory,current=null,baseline='',saving=false,opening=false,confirmResolve=null;
 const bytes=s=>new TextEncoder().encode(s).length;
 const el=(tag,cls,text)=>{const n=document.createElement(tag);if(cls)n.className=cls;if(text!==undefined)n.textContent=text;return n;};
 function button(text,label,fn){const b=el('button','',text);b.type='button';b.setAttribute('aria-label',label);b.title=label;b.addEventListener('click',fn);return b;}

 let codeEditor=null,reviewChanges,editorRetry=0;
 async function loadEditor(epoch){
  if(codeEditor)return;
  const url=new URL('./editor.mjs',import.meta.url);
  if(editorRetry)url.searchParams.set('retry',String(editorRetry));
  try{
   const module=await import(url.href);
   if(epoch!==generation)return;
   codeEditor=module.createEditor(input,save);
   reviewChanges=module.reviewChanges;
  }catch{
   editorRetry++;
   throw Error('编辑器加载失败，请重新打开文件');
  }
 }
 let nameResolve=null;
 const tools=el('div','file-navigation file-tools'),uploadInput=el('input');uploadInput.type='file';uploadInput.hidden=true;
 const uploadButton=button('上传','上传文件',()=>{if(filesAvailable)uploadInput.click();}),newFileButton=button('新建文件','新建文件',createFile),mkdirButton=button('新建目录','新建目录',()=>changeName('mkdir',root));tools.append(uploadButton,newFileButton,mkdirButton,uploadInput);$('#file-location-form').after(tools);

 const nameDialog=el('dialog','node-dialog file-name-dialog'),nameBody=el('div','dialog-inner'),nameTitle=el('h2'),nameForm=el('form','settings-form'),nameLabel=el('label','form-field','名称'),nameInput=el('input'),nameError=el('p','form-error'),nameSubmit=el('button','primary-button','确定'),nameCancel=button('取消','取消',()=>finishName(null));nameInput.required=true;nameInput.maxLength=255;nameInput.autocomplete='off';nameSubmit.type='submit';nameLabel.append(nameInput);nameForm.append(nameLabel,nameError,nameSubmit,nameCancel);nameBody.append(nameTitle,nameForm);nameDialog.append(nameBody);scope.append(nameDialog);
 function finishName(value){if(nameDialog.open)nameDialog.close();const r=nameResolve;nameResolve=null;r?.(value);}
 function askName(title,value=''){if(nameResolve)return Promise.resolve(null);nameTitle.textContent=title;nameInput.value=value;nameError.textContent='';nameDialog.showModal();nameInput.focus();nameInput.select();return new Promise(resolve=>nameResolve=resolve);}
 nameForm.onsubmit=e=>{e.preventDefault();const v=nameInput.value;if(!v||v==='.'||v==='..'||/[/\x00-\x1f\x7f]/.test(v)||bytes(v)>255){nameError.textContent='名称不能含斜线或控制字符，最多 255 字节';return;}finishName(v);};nameDialog.oncancel=e=>{e.preventDefault();finishName(null);};
 async function changeName(action,path){if(!filesAvailable||saving)return;if(!await confirmDiscard())return;const value=await askName(action==='mkdir'?'新建目录':'重命名',action==='rename'?path.split('/').pop():'');if(value===null)return;try{await request(action,path,{target:value});cache.clear();if(action==='rename'&&current&&(current.path===path||current.path.startsWith(path+'/'))){closeEditor();}await load(root,{navigate:true});treeNote(action==='mkdir'?'目录已创建':'名称已更新',false,action==='mkdir');}catch(e){treeNote(e.message,true);}}
 async function createFile(){if(!filesAvailable||saving||opening)return;const epoch=generation,path=root,value=await askName('新建文件');if(value===null||epoch!==generation||!filesAvailable)return;await queue.add(new File([],value,{type:'text/plain'}),path,true);}
 const queue=createTransfers({holder:$('#terminal-file-pane'),request,note:treeNote,refresh:async()=>{cache.delete(root);await load(root,{navigate:true});}});
 async function perform(file,path,creating=false){return queue.add(file,path,creating);}
 uploadInput.multiple=true;
 uploadInput.onchange=()=>{const files=[...uploadInput.files];uploadInput.value='';for(const file of files)perform(file,root);};
 function icon(kind){const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');svg.setAttribute('viewBox','0 0 20 20');svg.setAttribute('aria-hidden','true');const path=document.createElementNS(svg.namespaceURI,'path');path.setAttribute('d',kind==='directory'?'M2.5 5.5h5l2-2h7.5v13h-15z':kind==='link'?'M8 12l4-4M7 9l-1 1a3 3 0 004 4l1-1M9 7l1-1a3 3 0 014 4l-1 1':'M5 2.5h7l3 3v12H5zM12 2.5v4h3M7.5 10h5M7.5 13h5');svg.append(path);return svg;}
 function dirty(){return !!current&&!current.readOnly&&input.value!==baseline;}
 function note(message,error=false){status.textContent=message;status.classList.toggle('is-error',error);}
 function summary(){if(!current)return;const size=bytes(input.value)+(current.lineEnding==='crlf'?(input.value.match(/\n/g)||[]).length:0),lines=input.value.split('\n').length;$('#file-meta').textContent=`UTF-8 · ${current.lineEnding.toUpperCase()} · ${lines} 行 · ${(size/1024).toFixed(1)} KiB`;$('#file-save').disabled=!filesAvailable||current.readOnly||saving||!current.revision||!dirty()||size>262144;$('#file-reload').disabled=!filesAvailable||saving;$('#file-dirty').hidden=!dirty();if(size>262144)note('内容超过 256 KiB，请缩小后保存。',true);}
 function view(which){const file=which==='file'&&!!current;scope.querySelector('.terminal-tabs').hidden=!current;editor.hidden=!file;$('#terminal-panel').hidden=file;$('#terminal-tab').classList.toggle('active',!file);$('#file-tab').classList.toggle('active',file);$('#terminal-tab').setAttribute('aria-selected',String(!file));$('#file-tab').setAttribute('aria-selected',String(file));if(!file){requestAnimationFrame(()=>{onResize();onTerminal();});}}
 function request(action,path,extra={}){const inspection=isInspection(action);if(!active||!inspection&&!filesAvailable)return Promise.reject(Error(inspection?'SSH 连接已断开，请重新连接':'文件连接尚未就绪，请稍后重试'));if([...pending.values()].filter(p=>p.inspection===inspection).length>=(inspection?2:3))return Promise.reject(Error(inspection?'资源查询正在进行':'已有文件操作正在进行'));const id='file_'+(++sequence);return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(Error('操作超时；若正在保存，请重新读取确认结果'));},['transfer_resume','upload_finish'].includes(action)?130000:action==='save'?50000:24000);pending.set(id,{resolve,reject,timer,inspection});try{send(JSON.stringify({type:'file',id,action,path,...extra}),action);}catch(e){clearTimeout(timer);pending.delete(id);reject(e);}});}

 function receive(message){const job=pending.get(message.id);if(!job)return;pending.delete(message.id);clearTimeout(job.timer);if(message.ok)job.resolve(message.data);else{const e=Error(message.error||'文件操作未完成');e.code=message.code;job.reject(e);}}
 function treeNote(message,error=false,dismissible=false){
  const status=$('#file-tree-status');status.textContent=message;status.classList.toggle('is-error',error);
  if(dismissible)status.append(' ',button('关闭','关闭目录创建提示',()=>treeNote('')));
 }
 let volumes=[],navigation=0;
 let bottomRevision=0;
 const bottomPanes=[['file','#terminal-file-pane'],['command','#terminal-command-pane'],['route','#terminal-route']];
 function bottom(which){
  bottomRevision++;
  for(const [name,pane]of bottomPanes){const selected=which===name;$(pane).hidden=!selected;const tab=$('#terminal-'+name+'-tab');tab.setAttribute('aria-selected',String(selected));tab.tabIndex=selected?0:-1;}
  requestAnimationFrame(onResize);
 }
 function selectBottom(which){bottom(which);if(which==='file'&&!cache.has(root))load(root,{navigate:true});if(which==='command')onCommands();}
 for(const [name]of bottomPanes)$('#terminal-'+name+'-tab').onclick=()=>selectBottom(name);
 scope.querySelector('.terminal-bottom-tabs').addEventListener('keydown',event=>{
  const index=bottomPanes.findIndex(([name])=>event.target===$('#terminal-'+name+'-tab'));
  if(index<0||!['ArrowLeft','ArrowRight','Home','End'].includes(event.key))return;
  event.preventDefault();const next=event.key==='Home'?0:event.key==='End'?2:(index+(event.key==='ArrowRight'?1:2))%3;
  selectBottom(bottomPanes[next][0]);$('#terminal-'+bottomPanes[next][0]+'-tab').focus();
 });

 async function load(path=root,{navigate=false,offset=0,action='list',selectPane=true}={}){
  const epoch=generation,nav=navigate?++navigation:navigation,paneRevision=bottomRevision;treeNote('正在读取…');
  try{const data=await request(action,path,{offset});if(epoch!==generation)return;
   const previous=cache.get(data.path);cache.set(data.path,{...data,entries:offset&&previous?[...previous.entries,...data.entries]:data.entries});
   if(navigate&&nav===navigation){root=data.path;pathInput.value=root;if(selectPane&&paneRevision===bottomRevision)bottom('file');}
   render();treeNote((cache.get(root)?.total||0)+' 项');
  }catch(e){if(epoch===generation)treeNote(e.message,true);}
 }
 function renderVolumes(){
  tree.replaceChildren();
  for(const item of volumes.length?volumes:[{path:'/'}]){
   const row=el('div','terminal-volume'),target=button('', '打开目录 '+item.path,()=>load(item.path,{navigate:true}));
   target.className='file-tree-target';target.append(el('span','file-tree-name',item.path));
   const sizes=el('span','file-volume-size',compactBytes(item.available)+' / '+compactBytes(item.total));sizes.title='可用容量 / 总容量';
   target.append(sizes);
   const track=el('div','terminal-volume-track'),fill=el('i');
   const percent=item.total>0?Math.max(0,Math.min(100,item.used/item.total*100)):0;
   fill.style.width=percent+'%';track.append(fill);track.title=Number.isFinite(item.used)?'已用 '+compactBytes(item.used):'容量暂不可用';
   row.append(target,track);tree.append(row);
  }
  $('#file-root-status').textContent=volumes.length?'':'磁盘容量暂不可用';
 }
 function setVolumes(value){volumes=Array.isArray(value)?value.filter(v=>typeof v.path==='string'&&v.path.startsWith('/')&&Number.isFinite(v.total)&&v.total>0).slice(0,128):[];renderVolumes();}
 function render(){
  const holder=$('#file-list');holder.replaceChildren();const data=cache.get(root);if(!data)return;
  const table=el('table','terminal-file-table'),head=el('thead'),hr=el('tr'),body=el('tbody');
  for(const label of ['名称','大小','修改时间','权限','操作'])hr.append(el('th','',label));head.append(hr);
  for(const item of data.entries){
   const row=el('tr'),name=el('td'),target=button('',(item.kind==='directory'?'打开目录 ':'打开文件 ')+item.name,()=>item.kind==='directory'?load(item.path,{navigate:true}):open(item.path));
   target.className='file-tree-target';target.append(icon(item.kind),el('span','file-tree-name',item.name));name.append(target);
   const actions=el('td','file-table-actions');actions.append(button('重命名','重命名 '+item.name,()=>changeName('rename',item.path)));
   if(item.kind!=='directory'&&item.kind!=='other')actions.append(button('下载','下载 '+item.name,()=>perform(null,item.path)));
   row.append(name,el('td','',item.kind==='directory'?'—':compactBytes(item.size)),el('td','',item.modified?new Date(item.modified*1000).toLocaleString('zh-CN',{hour12:false}):'—'),el('td','file-mode',item.mode),actions);body.append(row);
  }table.append(head,body);holder.append(table);
  if(!data.entries.length)holder.append(el('p','file-empty','目录为空'));
  if(data.nextOffset>=0)holder.append(button('加载更多','加载更多当前目录',()=>load(root,{offset:data.nextOffset})));
 }
 const findButton=button('查找替换','查找与替换文件内容',()=>codeEditor?.search());$('#file-wrap').closest('label').before(findButton);
 const discard=$('#file-discard-dialog');
 function resolveDiscard(value){if(discard.open)discard.close();const resolve=confirmResolve;confirmResolve=null;resolve?.(value);}
 async function confirmDiscard(keep=false){if(saving){note('正在保存，请等待操作完成。',true);return false;}if(!dirty()&&(keep||!queue.isBusy()))return true;$('#file-discard-title').textContent=queue.isBusy()&&!keep?'文件正在传输':'文件修改尚未保存';discard.querySelector('p').textContent=queue.isBusy()&&!keep?'结束会话会取消队列，并丢弃未保存的编辑内容。':'离开后会丢弃当前编辑内容。';$('#file-discard-confirm').textContent=queue.isBusy()&&!keep?'取消传输并离开':'放弃修改';if(confirmResolve)return false;discard.showModal();return new Promise(resolve=>{confirmResolve=resolve;$('#file-keep-editing').focus();});}
 $('#file-keep-editing').addEventListener('click',()=>resolveDiscard(false));$('#file-discard-confirm').addEventListener('click',()=>resolveDiscard(true));discard.addEventListener('cancel',e=>{e.preventDefault();resolveDiscard(false);});
 function show(data){current=data;input.value=data.content;baseline=input.value;input.readOnly=data.readOnly;input.disabled=false;$('#file-name').textContent=data.path;$('#file-name').title=data.path;$('#file-tab-label').textContent=data.path.split('/').pop()||'/';$('#file-tab').hidden=false;$('#file-close').hidden=false;note(data.readOnly?data.reason:'');summary();codeEditor.sync(data.path,true);view('file');}
 async function open(path){if(opening||saving)return;const epoch=generation;if(!await confirmDiscard()||opening||saving||epoch!==generation)return;opening=true;treeNote('正在读取文件…');try{const data=await request('read',path);if(epoch!==generation)return;if(data.kind==='directory'){root=data.path;pathInput.value=root;cache.set(root,data);bottom('file');render();treeNote(data.total+' 项');return;}await loadEditor(epoch);if(epoch!==generation)return;show(data);treeNote('');}catch(e){if(epoch===generation){treeNote(e.message,true);if(!editor.hidden)note(e.message,true);}}finally{if(epoch===generation)opening=false;}}
 function closeEditor(){
  current=null;baseline='';input.value='';codeEditor?.sync();
  $('#file-tab').hidden=true;$('#file-close').hidden=true;$('#file-dirty').hidden=true;
  $('#file-name').textContent='';$('#file-meta').textContent='';note('');view('terminal');
 }
 async function save(){
  if(!current||!dirty()||saving||current.readOnly||!filesAvailable)return;
  const epoch=generation,target=current,payload=input.value;
  saving=true;summary();
  try{
   if(!await reviewChanges(scope,baseline,payload,target.path)||epoch!==generation||current!==target||!filesAvailable)return;
   input.disabled=true;codeEditor?.sync();note('正在保存…');
   const data=await request('save',target.requestedPath,{revision:target.revision,content:payload});
   if(epoch!==generation||current!==target)return;
   if(input.value===payload){closeEditor();treeNote('已保存 · '+target.path);}
   else{current=data;baseline=payload;note('已保存提交的内容，后续修改尚未保存');}
  }catch(e){if(epoch===generation)note(e.message,true);}
  finally{if(epoch===generation){saving=false;input.disabled=false;codeEditor?.sync();summary();}}
 }
 $('#file-save').addEventListener('click',save);$('#file-reload').addEventListener('click',()=>current&&open(current.requestedPath));input.addEventListener('input',()=>{if(!saving)note(dirty()?'尚未保存':'');summary();});input.addEventListener('keydown',e=>{if((e.ctrlKey||e.metaKey)&&e.key.toLowerCase()==='s'){e.preventDefault();save();}if(e.key==='Tab'&&!e.shiftKey&&!e.ctrlKey&&!e.metaKey&&!input.readOnly){e.preventDefault();input.setRangeText('  ',input.selectionStart,input.selectionEnd,'end');input.dispatchEvent(new Event('input'));}});
 $('#file-wrap').addEventListener('change',e=>{input.wrap=e.target.checked?'soft':'off';codeEditor?.wrap(e.target.checked);});
 $('#file-location-form').addEventListener('submit',e=>{e.preventDefault();const value=pathInput.value.trim();if(value.startsWith('/'))load(value,{navigate:true});else treeNote('请输入以 / 开头的完整路径',true);});
 $('#file-root').addEventListener('click',()=>load('/',{navigate:true}));$('#file-home').addEventListener('click',()=>load('/',{navigate:true,action:'home'}));$('#file-up').addEventListener('click',()=>load(root.split('/').slice(0,-1).join('/')||'/',{navigate:true}));$('#file-refresh').addEventListener('click',()=>{cache.delete(root);load(root,{navigate:true});});
 $('#terminal-tab').addEventListener('click',()=>view('terminal'));$('#file-tab').addEventListener('click',()=>view('file'));$('#file-close').addEventListener('click',async()=>{if(!await confirmDiscard())return;closeEditor();});
 const separator=$('#file-resizer');let resizing=false;const width=value=>{const n=workspaceWidth(value,dialog.clientWidth);scope.style.setProperty('--file-width',n+'px');separator.setAttribute('aria-valuenow',String(Math.round(n)));separator.setAttribute('aria-valuemin',dialog.clientWidth<=700?'150':'220');separator.setAttribute('aria-valuemax',String(workspaceWidth(99999,dialog.clientWidth)));onResize();};separator.addEventListener('pointerdown',e=>{resizing=true;separator.setPointerCapture(e.pointerId);e.preventDefault();});separator.addEventListener('pointermove',e=>{if(resizing)width(e.clientX-dialog.getBoundingClientRect().left);});separator.addEventListener('pointerup',()=>{resizing=false;});separator.addEventListener('lostpointercapture',()=>{resizing=false;});separator.addEventListener('keydown',e=>{if(['ArrowLeft','ArrowRight'].includes(e.key)){e.preventDefault();width(sidebar.clientWidth+(e.key==='ArrowLeft'?-20:20));}});
 const bottomSeparator=$('#terminal-bottom-resizer'),main=$('.terminal-main');let bottomResize=false;
 function height(value){const n=workspaceHeight(value,main.clientHeight);scope.style.setProperty('--bottom-height',n+'px');bottomSeparator.setAttribute('aria-valuenow',String(Math.round(n)));bottomSeparator.setAttribute('aria-valuemax',String(Math.max(150,main.clientHeight-240)));onResize();}
 bottomSeparator.onpointerdown=e=>{bottomResize=true;bottomSeparator.setPointerCapture(e.pointerId);e.preventDefault();};
 bottomSeparator.onpointermove=e=>{if(bottomResize)height(main.getBoundingClientRect().bottom-e.clientY);};
 bottomSeparator.onpointerup=bottomSeparator.onlostpointercapture=()=>{bottomResize=false;};
 bottomSeparator.onkeydown=e=>{if(['ArrowUp','ArrowDown'].includes(e.key)){e.preventDefault();height($('#terminal-bottom').clientHeight+(e.key==='ArrowUp'?20:-20));}};
 let narrowViewport;const sizeObserver=new ResizeObserver(()=>{if(!scope.isConnected)return;const narrow=dialog.clientWidth<=700;width(narrow&&narrowViewport!==true?180:sidebar.clientWidth||254);narrowViewport=narrow;height($('#terminal-bottom').clientHeight||208);});sizeObserver.observe(main);
 const beforeUnload=e=>{if(dirty()||queue.isBusy()){e.preventDefault();e.returnValue='';}};window.addEventListener('beforeunload',beforeUnload);
 function clear(){uploadButton.disabled=true;mkdirButton.disabled=true;newFileButton.disabled=true;finishName(null);generation++;active=false;filesAvailable=false;saving=false;opening=false;current=null;baseline='';input.value='';input.disabled=false;input.readOnly=false;cache.clear();tree.replaceChildren();root=defaultDirectory;pathInput.value=root;volumes=[];renderVolumes();bottom('file');$('#file-list').replaceChildren();for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('SSH 连接已结束'));}pending.clear();$('#file-tab').hidden=true;$('#file-close').hidden=true;$('#file-name').textContent='';$('#file-meta').textContent='';codeEditor?.sync();note('');treeNote('连接 SSH 后浏览文件');resolveDiscard(false);view('terminal');}
 function suspendFiles(){filesAvailable=false;generation++;opening=false;saving=false;for(const [id,p]of pending){if(!p.inspection){clearTimeout(p.timer);p.reject(Error('文件连接中断，队列已暂停'));pending.delete(id);}}queue.suspend();uploadButton.disabled=mkdirButton.disabled=newFileButton.disabled=true;input.disabled=true;codeEditor?.sync();summary();treeNote('文件连接中断，重新连接后可继续传输');}
 function suspend(){active=false;suspendFiles();for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('SSH 连接中断'));}pending.clear();}

 function ready(supported,context={}){if(!supported){treeNote('当前主控版本尚未启用文件功能');return;}active=true;filesAvailable=true;uploadButton.disabled=false;mkdirButton.disabled=false;newFileButton.disabled=false;input.disabled=false;codeEditor?.sync();queue.active(context);renderVolumes();load(root,{navigate:true,selectPane:false});if(current){const target=current,epoch=generation;request('read',target.requestedPath).then(data=>{if(current!==target||epoch!==generation)return;if(data.kind==='file'&&data.content.replace(/\r\n/g,'\n')===baseline.replace(/\r\n/g,'\n')){current={...data};input.readOnly=data.readOnly;note('连接已恢复，远端版本已核对');}else{current.revision='';note('远端文件已变化，当前修改已保留。请复制修改后重新读取并合并。',true);}codeEditor?.sync();summary();}).catch(e=>{if(current===target&&epoch===generation){current.revision='';note(e.message,true);summary();}});}}

 clear();return {request,setVolumes,connectionReady:()=>{active=true;},suspendFiles,status:treeNote,hasTransfers:()=>queue.isBusy(),ready,receive,clear,suspend,end:()=>{queue.reset();clear();},confirmDiscard,showTerminal:()=>view('terminal'),isDirty:()=>dirty()||queue.isBusy(),destroy:(options={})=>{sizeObserver.disconnect();queue.destroy(options);clear();codeEditor?.destroy();window.removeEventListener('beforeunload',beforeUnload);}};
}
