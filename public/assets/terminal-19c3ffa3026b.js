import {createTerminalMonitor,createRouteView} from "/assets/terminal-monitor-167d3dfe6356.js";
import {terminalAppearance} from "/assets/terminal-appearance-57b3a886f0d5.js";
import {TerminalRetry,canRetryRequest,canRetryClose} from "/assets/terminal-retry-9f33960c93a8.js";
import {createTerminalKeys} from "/assets/terminal-keys-54aad77e70fd.js";
import {SearchAddon} from "/assets/search-3ea90162233f.js";
import {createFileChannel} from "/assets/terminal-file-channel-fa0fc483f8bb.js";
import {isInspection,createFileBrowser} from "/assets/files-c0e16017194a.js";
import {findTransferContext} from "/assets/transfers-e30ce1961db2.js";
const transferClaims=new Set();
import {Terminal} from "/assets/xterm-b336ec65a086.js";
import {FitAddon} from "/assets/fit-2d87e1bddc73.js";
function createSession(api,root,dialog,onClose,onState,getRecord){
 const $=s=>root.querySelector(s),mount=$('#terminal-mount'),placeholder=$('.terminal-placeholder');
 let separateFiles=false,socket=null,term=null,fit=null,search=null,sendInput=()=>{},resize=null,touch=null,current=null,generation=0,connected=false,transferSession='',shellSession='',lookedForTransfers=false;
 function claimFiles(id){if(transferSession)transferClaims.delete(transferSession);transferSession=id||'';if(transferSession)transferClaims.add(transferSession);}
 async function end(force=false){reconnect.reset();const id=shellSession;if(socket?.readyState===WebSocket.OPEN)socket.send(JSON.stringify({type:'close'}));stop(true);if(id&&!force){try{await api('/api/admin/terminal-close','POST',{id},{keepalive:true});}catch(e){if(![401,404].includes(e.status)&&!e.staleSession){hint('结束会话尚未确认，网络恢复后请重试；脱离连接的会话将在 5 分钟后清理。');throw e;}}}shellSession='';files.end?.();claimFiles('');}

 const reconnect=new TerminalRetry({retry:()=>{if(current&&dialog.open)start(true);},waiting:(delay,attempt,total)=>{state('等待重连');hint('连接中断，'+delay/1000+' 秒后重连（'+attempt+'/'+total+'）。将恢复原 SSH 会话。');}});
 const state=(value,ready=false)=>{$('#terminal-state').textContent=value;onState(ready,value);};
 const hint=value=>{$('#terminal-hint').textContent=value;$('#terminal-footer-state').textContent=value;};
 function stop(clear=false){fileChannel.stop();monitor.stop();route.stop();reconnect.cancel();keys.ready(false);sendInput=()=>{};files.suspend?.();closePaste();commandReady(false);generation++;connected=false;if(socket){socket.onclose=null;socket.close(1000,'closed');socket=null;}resize?.disconnect();resize=null;state('已断开');if(clear){touch?.();touch=null;term?.dispose();term=null;fit=null;mount.replaceChildren();mount.hidden=true;placeholder.hidden=false;hint('关闭标签后可重新连接。');}else if(term){term.options.disableStdin=true;}}
 function open(record){stop(true);current=record;$('#terminal-title').textContent=record.public.name;$('#terminal-target').textContent=record.username+'@'+record.ip+':'+record.port;state('未连接');hint(record.demo?'演示节点暂不支持 SSH。':record.public.online?'正在连接…':'节点暂未在线，请在上线后重新打开终端。');if(!dialog.open)dialog.showModal();loadCommands();if(!record.demo)start();}
 async function start(automatic=false){if(!current||!dialog.open)return;const retrying=automatic===true;if(!retrying)reconnect.reset();if(!lookedForTransfers){lookedForTransfers=true;claimFiles(findTransferContext(current.public.id,transferClaims));}if(transferSession&&!files.hasTransfers()&&connected)claimFiles('');stop(true);const run=generation;hint('正在请求终端授权…');state('连接中');
  try{const result=await api('/api/admin/terminal-ticket','POST',{id:current.public.id});if(run!==generation||!dialog.open)return;
   term=new Terminal({fontFamily:'Consolas, "Cascadia Mono", "SFMono-Regular", Menlo, "DejaVu Sans Mono", "Liberation Mono", monospace',fontSize:13,lineHeight:1.25,minimumContrastRatio:4.5,scrollback:5000,cursorBlink:true,allowProposedApi:false,disableStdin:true,allowTransparency:true,theme:terminalAppearance()});fit=new FitAddon();term.loadAddon(fit);search=new SearchAddon();term.loadAddon(search);
   // Block terminal-controlled links, clipboard access and window operations.
   term.parser.registerOscHandler(52,()=>true);term.parser.registerOscHandler(8,()=>true);term.parser.registerOscHandler(0,()=>true);term.parser.registerOscHandler(2,()=>true);
   placeholder.hidden=true;mount.hidden=false;term.open(mount);let searchShortcut=false;term.attachCustomKeyEventHandler(e=>{if(e.type==='keydown'&&(e.ctrlKey||e.metaKey)&&e.shiftKey&&e.key.toLowerCase()==='f'){searchShortcut=true;e.preventDefault();return false;}if(e.type==='keyup'&&searchShortcut&&e.key.toLowerCase()==='f'){searchShortcut=false;keys.search();e.preventDefault();return false;}return true;});fit.fit();touch=terminalTouch(term);
   socket=new WebSocket(new URL('/api/terminal',location.href).href.replace(/^https:/,'wss:'));socket.binaryType='arraybuffer';const ws=socket;let failure='',retryDecision=null;
   const send=(data)=>{if(ws.readyState===WebSocket.OPEN)ws.send(data);};
   const input=data=>{if(!connected||ws.readyState!==WebSocket.OPEN)return;if(ws.bufferedAmount>128*1024){stop();hint('输入过快，连接已关闭。');return;}for(let i=0;i<data.length;i+=16384)send(data.slice(i,i+16384));};
   sendInput=value=>input(new TextEncoder().encode(value));term.onData(value=>sendInput(keys.transform(value)));term.onBinary(value=>input(Uint8Array.from(value,c=>c.charCodeAt(0)&255)));
   term.onResize(({cols,rows})=>{if(connected)send(JSON.stringify({type:'resize',cols,rows}));});resize=new ResizeObserver(()=>{if(dialog.open&&term&&mount.getClientRects().length)fit.fit();});resize.observe(mount);
   ws.onopen=()=>{if(run===generation&&dialog.open)send(JSON.stringify({type:'authorize',ticket:result.ticket,transferSession,shellSession,cols:term.cols,rows:term.rows}));};
   ws.onmessage=event=>{if(run!==generation)return;if(typeof event.data==='string'){let message;try{message=JSON.parse(event.data);}catch{stop();return;}if(message.type==='session'){shellSession=message.shellSession||'';return;}if(message.type==='file_result'){files.receive(message);return;}if(message.type==='ready'){connected=true;reconnect.connected();claimFiles(message.transferSession);shellSession=message.shellSession||'';keys.ready(message.terminal!==false);separateFiles=message.fileChannel===true;if(separateFiles){files.connectionReady();fileChannel.open({session:transferSession,node:current.public.id});}else files.ready(message.files===true,{session:transferSession,node:current.public.id});hint(message.restored?'已恢复原 SSH 会话；此前的输入不会重复发送。':'异常断线保留会话 5 分钟；关闭标签即结束。');term.options.disableStdin=message.terminal===false;commandReady(message.terminal!==false);state('已连接',true);monitor.start();route.start();hint(message.restored?'已恢复原会话':'');term.focus();}else if(message.type==='error'){retryDecision=message.retryable===true;if(!connected&&!shellSession)claimFiles('');if(message.status===410){shellSession='';}failure=message.error||message.message||'连接失败';state(failure);term.write(connected?'\r\n终端连接已中断，请查看下方提示。\r\n':'\r\n连接未完成，请重新连接。\r\n');}else if(message.type==='notice'){term.write('\r\n'+message.message+'\r\n');}else if(message.type==='closed'){retryDecision=false;shellSession='';failure=message.message||'远端 SSH 会话已结束';state('会话已结束');}}else{term.write(new Uint8Array(event.data),()=>send(JSON.stringify({type:'ack'})));}};
   ws.onerror=()=>state('连接异常');ws.onclose=event=>{if(run!==generation)return;fileChannel.stop();monitor.stop();route.stop();reconnect.cancel();connected=false;keys.ready(false);sendInput=()=>{};files.suspend?.();closePaste();commandReady(false);term.options.disableStdin=true;state('已断开');if(!files.hasTransfers())claimFiles('');hint(failure||(shellSession?'连接中断，正在尝试恢复原会话。':'会话已结束，请关闭此标签后新建连接。'));resize?.disconnect();if(canRetryClose(event.code,retryDecision)&&dialog.open){if(!reconnect.schedule())hint((failure||'连接中断')+'；自动重试已结束，请关闭此标签后重新连接。');}};
  }catch(error){if(run===generation&&dialog.open){hint(error.message);state('连接失败');if(canRetryRequest(error)&&!reconnect.schedule())hint(error.message+'；自动重试已结束，请关闭此标签后重新连接。');}}
 }
 let commandLoad=0,commandDisposed=false,commandAvailable=false;
 function commandReady(value){commandAvailable=value;for(const b of root.querySelectorAll('.terminal-command-run'))b.disabled=!value;}
 async function loadCommands(){
  const run=++commandLoad,list=$('#terminal-command-list'),status=$('#terminal-command-status');
  status.textContent='正在加载常用命令…';
  try{
   const result=await api('/api/admin/commands');
   if(commandDisposed||run!==commandLoad||!dialog.open)return;
   list.replaceChildren();
   for(const command of result.commands){
    const row=document.createElement('div'),name=document.createElement('span'),button=document.createElement('button');
    row.className='terminal-command-item';name.className='terminal-command-name';name.textContent=command.name;name.title=command.name;
    button.type='button';button.className='terminal-command-run';button.textContent='运行';button.setAttribute('aria-label','运行 '+command.name);button.disabled=!commandAvailable;
    button.addEventListener('click',()=>{
     if(!commandAvailable||!connected||socket?.readyState!==WebSocket.OPEN)return;
     if(socket.bufferedAmount>128*1024){hint('正在发送其他内容，请稍后重试。');return;}
     socket.send(JSON.stringify({type:'command',id:command.id}));files.showTerminal();term?.focus();
    });
    row.append(name,button);list.append(row);
   }
   status.textContent=result.commands.length?'':'暂无常用命令，可在管理后台添加。';
  }catch(e){if(!commandDisposed&&run===commandLoad){list.replaceChildren();status.textContent=e.message||'常用命令加载失败，请重新打开命令页重试。';}}
 }

 function closePaste(){const d=$('#terminal-paste-dialog');if(d.open)d.close();$('#terminal-paste-input').value='';$('#paste-error').textContent='';}
 function openPaste(){if(!connected||term?.options.disableStdin)return;$('#paste-error').textContent='';$('#terminal-paste-dialog').showModal();$('#terminal-paste-input').focus();}
 $('#paste-close').addEventListener('click',closePaste);$('#paste-cancel').addEventListener('click',closePaste);$('#terminal-paste-dialog').addEventListener('close',()=>{$('#terminal-paste-input').value='';});
 $('#terminal-paste-form').addEventListener('submit',event=>{event.preventDefault();const script=$('#terminal-paste-input').value.replace(/\r\n?/g,'\n').trimEnd();const bytes=new TextEncoder().encode(script+'\n');if(!script.trim()){return;}if(bytes.length>8192||/[\x00-\x08\x0b-\x1f\x7f]/.test(script)){$('#paste-error').textContent='命令最多 8192 字节，且不能含特殊控制字符。';return;}if(!connected||socket?.readyState!==WebSocket.OPEN){$('#paste-error').textContent='连接已断开，请重新连接。';return;}if(socket.bufferedAmount>128*1024){$('#paste-error').textContent='正在发送其他内容，请稍后重试。';return;}socket.send(bytes);closePaste();term.focus();});
 $('#terminal-close').addEventListener('click',onClose);$('#terminal-maximize').addEventListener('click',()=>{const active=dialog.classList.toggle('is-expanded');$('#terminal-maximize').setAttribute('aria-pressed',String(active));$('#terminal-maximize').textContent=active?'还原':'展开';requestAnimationFrame(()=>fit?.fit());});
 const keys=createTerminalKeys({root,getTerm:()=>term,getSearch:()=>search,send:value=>sendInput(value),paste:openPaste,fit:()=>requestAnimationFrame(()=>fit?.fit())});
 const files=createFileBrowser({dialog,scope:root,onCommands:loadCommands,send:(value,action)=>{if(separateFiles&&!isInspection(action)){fileChannel.send(value);return;}if(!connected||socket?.readyState!==WebSocket.OPEN)throw Error('SSH 连接已断开');if(socket.bufferedAmount>128*1024)throw Error('正在发送其他内容，请稍后重试');socket.send(value);},onResize:()=>{if(term&&mount.getClientRects().length)fit?.fit();},onTerminal:()=>{if(connected)term?.focus();}});
 const fileChannel=createFileChannel({api,onReady:context=>{if(connected)files.ready(true,context);},onMessage:message=>files.receive(message),onDisconnect:()=>files.suspendFiles(),onStatus:message=>files.status(message)});
 const monitor=createTerminalMonitor({root,getRecord,connected:()=>connected,request:files.request,onVolumes:files.setVolumes});
 const route=createRouteView({root,api,getRecord,connected:()=>connected,getTransfer:()=>transferSession});
 async function confirmClose(){reconnect.cancel();const discard=await files.confirmDiscard();if(!discard&&!connected){state('已断开');if(!reconnect.schedule())hint('自动重试已结束，请保留文件修改后关闭标签并重新连接。');}return discard;}
 return {open,confirm:confirmClose,fit:()=>{if(term&&mount.getClientRects().length)fit?.fit();},deactivate:()=>keys.reset(),refresh:()=>{monitor.refresh();route.refresh();},destroy:async(force=false)=>{await end(force);monitor.destroy();route.destroy();keys.destroy();files.destroy();commandDisposed=true;commandLoad++;current=null;}};
}

export function createTerminalUI(api,getRecords,toast){
 const dialog=document.querySelector('#terminal-dialog'),shell=dialog.querySelector('.terminal-shell').cloneNode(true);
 const paste=document.querySelector('#terminal-paste-dialog').cloneNode(true),discard=document.querySelector('#file-discard-dialog').cloneNode(true);
 document.querySelector('#terminal-paste-dialog').remove();document.querySelector('#file-discard-dialog').remove();
 const bar=document.createElement('div');bar.className='ssh-session-bar';const tabs=document.createElement('div');tabs.className='ssh-session-tabs';tabs.setAttribute('role','tablist');tabs.setAttribute('aria-label','SSH 会话');
 const add=document.createElement('button');add.type='button';add.className='ssh-add-session';add.textContent='+';add.setAttribute('aria-label','新建 SSH 会话');add.setAttribute('aria-expanded','false');const picker=document.createElement('div');picker.className='ssh-server-picker';picker.hidden=true;picker.setAttribute('role','group');picker.setAttribute('aria-label','选择服务器连接');
 const stage=document.createElement('div');stage.className='ssh-session-stage';bar.append(tabs,add,picker);dialog.replaceChildren(bar,stage);
 let active=null,next=0,closing=false;const sessions=new Map();
 const viewport=window.visualViewport;
 function sizeViewport(){if(!dialog.open)return;const mobile=matchMedia('(max-width:700px)').matches;dialog.classList.toggle('is-compact-terminal',mobile&&!!viewport&&viewport.height<500);if(mobile&&viewport){dialog.style.height=Math.max(180,viewport.height-16)+'px';dialog.style.maxHeight=Math.max(180,viewport.height-16)+'px';dialog.style.top=(viewport.offsetTop+8)+'px';dialog.style.marginTop='0';}else{for(const p of ['height','max-height','top','margin-top'])dialog.style.removeProperty(p);}requestAnimationFrame(()=>sessions.get(active)?.ui?.fit());}
 viewport?.addEventListener('resize',sizeViewport);viewport?.addEventListener('scroll',sizeViewport);window.addEventListener('resize',sizeViewport);
 function draw(){tabs.replaceChildren();let number=0;for(const [id,s]of sessions){number++;const wrap=document.createElement('div');wrap.className='ssh-session-tab'+(id===active?' active':'');const b=document.createElement('button');b.type='button';const dot=document.createElement('i');dot.className='ssh-tab-dot'+(s.ready?' connected':'');dot.setAttribute('aria-hidden','true');const label=document.createElement('span');label.textContent=s.record.public.name+' · '+number;b.append(dot,label);b.title=s.record.public.name+' · '+(s.state||'未连接');b.setAttribute('aria-label',b.title);b.setAttribute('role','tab');b.setAttribute('aria-selected',String(id===active));b.onclick=()=>activate(id);const x=document.createElement('button');x.type='button';x.textContent='×';x.setAttribute('aria-label','关闭会话 '+number);x.onclick=()=>remove(id);wrap.append(b,x);tabs.append(wrap);}add.disabled=sessions.size>=4;add.title='新建 SSH 会话（最多 4 个）';}
 function closePicker(){picker.hidden=true;add.setAttribute('aria-expanded','false');}
 function showPicker(){picker.replaceChildren();const records=getRecords().filter(r=>!r.removing&&r.public.online&&!r.demo);for(const r of records){const b=document.createElement('button');b.type='button';const name=document.createElement('strong'),ip=document.createElement('span');name.textContent=r.public.name;ip.textContent=r.currentIP||r.ip;ip.title=r.ip;b.append(name,ip);b.onclick=()=>{closePicker();open(r,true);};picker.append(b);}if(!records.length){const empty=document.createElement('p');empty.textContent='暂无在线服务器';picker.append(empty);}picker.hidden=false;add.setAttribute('aria-expanded','true');picker.querySelector('button')?.focus();}
 function activate(id){const s=sessions.get(id);if(!s)return;sessions.get(active)?.ui?.deactivate();active=id;stage.replaceChildren(s.root);sizeViewport();draw();requestAnimationFrame(()=>{s.ui.fit();s.ui.refresh();});}
 async function remove(id){const s=sessions.get(id);if(!s)return;activate(id);if(!await s.ui.confirm())return;try{await s.ui.destroy();}catch(e){toast(e.message,true);return;}sessions.delete(id);if(sessions.size)activate(sessions.keys().next().value);else{active=null;stage.replaceChildren();dialog.close();}draw();}
 function open(record,force=false){if(!force){const found=[...sessions].find(([,s])=>s.record.public.id===record.public.id);if(found){activate(found[0]);if(!dialog.open)dialog.showModal();return;}}if(sessions.size>=4){toast('最多同时打开 4 个 SSH 标签',true);return;}sessions.get(active)?.ui?.deactivate();const id=++next,root=document.createElement('div');root.className='ssh-session';root.append(shell.cloneNode(true),paste.cloneNode(true),discard.cloneNode(true));const s={root,record,ui:null};sessions.set(id,s);active=id;stage.replaceChildren(root);if(!dialog.open)dialog.showModal();s.ui=createSession(api,root,dialog,()=>remove(id),(ready,state)=>{s.ready=ready;s.state=state;draw();},()=>getRecords().find(r=>r.public.id===record.public.id)||record);s.ui.open(record);draw();sizeViewport();}
 async function close(force=false){for(const [id,s]of sessions){try{await s.ui.destroy(force);}catch(e){activate(id);toast('结束会话未确认：'+e.message,true);return;}sessions.delete(id);}active=null;stage.replaceChildren();dialog.close();draw();}
 // File-picker cancel events bubble; only this dialog may close the SSH workspace.
 dialog.addEventListener('cancel',async e=>{if(e.target!==dialog)return;e.preventDefault();if(!picker.hidden){closePicker();return;}if(closing)return;closing=true;try{for(const [id,s]of sessions){activate(id);if(!await s.ui.confirm())return;}await close();}finally{closing=false;}});
 dialog.addEventListener('close',e=>{if(e.target===dialog&&sessions.size)close(true);});
 add.onclick=()=>picker.hidden?showPicker():closePicker();
 dialog.addEventListener('pointerdown',e=>{if(!picker.contains(e.target)&&e.target!==add)closePicker();});
 dialog.addEventListener('keydown',e=>{if(e.key==='Escape'&&!picker.hidden){e.preventDefault();e.stopPropagation();closePicker();add.focus();}});
 window.addEventListener('pagehide',()=>{for(const s of sessions.values())s.ui.destroy().catch(()=>{});});
 return {open,close};
}

// xterm's desktop scrollbar handles wheels; touch scrolling is local and never
// sends arrow keys or mouse reports into a shell that is displaying history.
function terminalTouch(term){
 const element=term.element,screen=element.querySelector('.xterm-screen');
 let gesture=null;
 const enabled=()=>term.buffer.active.type==='normal'&&term.modes.mouseTrackingMode==='none';
 const start=event=>{gesture=event.touches.length===1&&enabled()?{id:event.touches[0].identifier,x:event.touches[0].clientX,y:event.touches[0].clientY,last:event.touches[0].clientY,pixels:0,scrolling:false}:null;};
 const move=event=>{
  if(!gesture||event.touches.length!==1||!enabled()){gesture=null;return;}
  const point=event.touches[0];if(point.identifier!==gesture.id)return;
  if(!gesture.scrolling){
   const dx=Math.abs(point.clientX-gesture.x),dy=Math.abs(point.clientY-gesture.y);
   if(Math.max(dx,dy)<8)return;
   if(dx>dy){gesture=null;return;}gesture.scrolling=true;
  }
  event.preventDefault();gesture.pixels+=gesture.last-point.clientY;gesture.last=point.clientY;
  const height=screen.clientHeight/term.rows;if(!Number.isFinite(height)||height<=0)return;
  const lines=Math.trunc(gesture.pixels/height);if(lines){gesture.pixels-=lines*height;term.scrollLines(lines);}
 };
 const end=()=>{gesture=null;};
 element.addEventListener('touchstart',start,{passive:true});
 element.addEventListener('touchmove',move,{passive:false});
 element.addEventListener('touchend',end,{passive:true});element.addEventListener('touchcancel',end,{passive:true});
 return()=>{end();element.removeEventListener('touchstart',start);element.removeEventListener('touchmove',move);element.removeEventListener('touchend',end);element.removeEventListener('touchcancel',end);};
}
