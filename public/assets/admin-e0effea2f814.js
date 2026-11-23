import {createServerManagement} from "/assets/server-management-c6b8bfe91b42.js";
import {showThemes} from "/assets/themes-0ffe18209439.js";
import {endAppearancePreview} from "/assets/appearance-40485caa5df3.js";
import {setupLeaseFields,leaseInput,money,cycleLabel,showBilling} from "/assets/billing-15dcd5a8ce66.js";
import {createInspectorUI} from "/assets/inspector-5d14e98d3224.js";
import {createOperations} from "/assets/operations-e10078a01a27.js";
import {authenticatorCanvas} from "/assets/authenticator-19965621ba87.js";
import {createTerminalUI} from "/assets/terminal-19c3ffa3026b.js";
import {countries as allCountries} from "/assets/countries-3dabb5bbec57.js";
import {createRenewalUI,formatExpiry,localExpiry} from "/assets/renewals-8bbd577cc65f.js";
export function initAdmin(hooks){
 const {el,icon,flag}=hooks,$=s=>document.querySelector(s);
 let forgetRemoval=false;
 let auth={authenticated:false},csrf='',records=[],site={name:'Vistart Probe',public:true,refreshSeconds:5},activeTab='overview',adminOpen=false,removeId=null,terminalId=null,commands=[],toastTimer,loading=false;
 const countries=Object.fromEntries([['OTHER','待识别'],...allCountries.map(c=>[c.code,c.name])]);
 const labels={enrollment_issued:'生成一次性接入命令',enrollment_claimed:'领取 Agent 接入配置',theme_updated:'修改站点主题',node_renewal_stopped:'停止服务器续费提醒',node_renewed:'确认服务器续费',file_read:'查看远程文件',file_saved:'保存远程文件',telegram_updated:'修改 Telegram 设置',telegram_test:'测试 Telegram 通知',telegram_sent:'Telegram 通知已发送',telegram_failed:'Telegram 通知发送失败',login:'管理员登录',login_failed:'登录验证失败',logout:'管理员退出',node_created:'添加服务器',node_updated:'更新服务器',node_deleted:'移除服务器',deploy_started:'开始部署',deploy_done:'部署完成',deploy_failed:'部署失败',ssh_host_trusted:'信任主机',ssh_host_replaced:'更新 SSH 主机指纹',terminal_opened:'连接 SSH',terminal_closed:'关闭 SSH',real_mode_enabled:'启用真实监控',command_created:'添加常用命令',command_updated:'修改常用命令',command_deleted:'移除常用命令',command_executed:'执行常用命令',site_updated:'修改面板设置',password_changed:'修改管理员密码',username_changed:'修改管理员用户名',mfa_enable:'启用二步验证',mfa_disable:'关闭二步验证',agent_connected:'Agent 上线',agent_disconnected:'Agent 离线',deploy_started:'开始部署 Agent',deploy_completed:'Agent 部署完成'};
 function toast(text,error=false){clearTimeout(toastTimer);$('#toast').textContent=text;$('#toast').classList.toggle('is-error',error);$('#toast').hidden=false;toastTimer=setTimeout(()=>$('#toast').hidden=true,6000);}
 let draggingNode=null;
 let sessionEpoch=0,sessionGate=null,siteEpoch=0;
 const sessionEndpoints=new Set(['/api/session','/api/login','/api/logout','/api/admin/password','/api/admin/username','/api/admin/mfa/enable','/api/admin/mfa/disable']);
 async function api(path,method='GET',body,extra={}){
  // Serialize cookie rotation, and discard responses issued under an older session.
  // A delayed monitoring poll must not sign out a freshly authenticated administrator.
  while(sessionGate)await sessionGate;
  const changesSession=sessionEndpoints.has(path);let release;
  if(changesSession){sessionEpoch++;sessionGate=new Promise(resolve=>{release=resolve;});}
  const epoch=sessionEpoch;
  try{
   const options={method,credentials:'same-origin',keepalive:extra.keepalive===true,headers:{'Content-Type':'application/json'}};
   if(method!=='GET'){options.headers['X-CSRF-Token']=csrf;if(body!==undefined)options.body=JSON.stringify(body);}
   let response;
   try{response=await fetch(path,options);}catch{const error=Error('连接主控失败，请稍后重试');error.network=true;throw error;}
   const data=await response.json().catch(()=>({error:'主控响应异常'}));
   if(epoch!==sessionEpoch){const error=Error('会话已更新，请重试');error.staleSession=true;throw error;}
   if(!response.ok){
    if(response.status===428){await operations.reauth();return api(path,method,body);}
    if(response.status===401&&path!=='/api/login')loseAuth();
    const error=Error(data.error||'请求失败');error.status=response.status;throw error;
   }
   if(changesSession&&typeof data.csrf==='string'){auth=data;csrf=data.csrf;authUI();}
   return data;
  }finally{if(release){sessionGate=null;release();}}
 }

 function closeDialog(id){const d=$(id);if(d?.open)d.close();}
 function wireDialog(dialog){for(const button of dialog.querySelectorAll('.dialog-dismiss')){button.replaceChildren(icon('close'));button.addEventListener('click',()=>dialog.close());}dialog.addEventListener('keydown',e=>{if(e.key!=='Tab')return;const controls=Array.from(dialog.querySelectorAll('button:not([disabled]),a[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])')).filter(e=>e.getClientRects().length>0);const first=controls[0],last=controls.at(-1);if(!first)return;if(controls.length===1||(e.shiftKey&&document.activeElement===first)||(!e.shiftKey&&document.activeElement===last)){e.preventDefault();(e.shiftKey?last:first).focus();}});}
 for(const id of ['#login-dialog','#node-editor','#delete-dialog','#terminal-dialog','#command-editor','#terminal-paste-dialog','#renewal-dialog'])wireDialog($(id));
 function authUI(){$('#login-mfa-field').hidden=!auth.mfaRequired;$('#login-form').elements.code.required=!!auth.mfaRequired;const admin=auth.authenticated;$('#admin-badge').hidden=!admin;$('#admin-entry').setAttribute('aria-label',admin?'打开管理后台':'管理员登录');$('#admin-entry').replaceChildren(icon('settings'));}
 function loseAuth(){enrollmentDialog.close();terminalUI.close(true);inspectorUI.close();operations.clear();renewalUI.clear();sessionEpoch++;auth={authenticated:false};csrf='';records=[];commands=[];serverUI.clear();$('#command-form').reset();adminOpen=false;$('#admin-view').hidden=true;$('#admin-view').replaceChildren();$('#main-content').hidden=false;for(const p of document.querySelectorAll('.admin-ip-row'))p.remove();for(const id of ['#node-editor','#delete-dialog','#terminal-dialog','#command-editor','#terminal-paste-dialog'])closeDialog(id);$('#node-form').reset();if($('#terminal-target'))$('#terminal-target').textContent='';authUI();hooks.onLogout?.();}
 let refreshQueued=false;
 function withCurrentIP(record){
  const currentIP=record.resolvedIPs?.length?record.resolvedIPs.join('、'):record.agentPeerIP||record.detectedIP||record.ip;
  return {...record,currentIP};
 }
 async function refresh(background=false){if(loading){if(!background)refreshQueued=true;return;}loading=true;try{while(sessionGate)await sessionGate;const asAdmin=auth.authenticated,epoch=sessionEpoch,siteVersion=siteEpoch;const result=await api(asAdmin?'/api/admin/nodes':'/api/public/nodes');if(epoch!==sessionEpoch)return;if(siteVersion===siteEpoch)site=result.site;records=asAdmin?result.nodes.map(withCurrentIP):[];const publicNodes=asAdmin?records.map(n=>({...n.public,demo:n.demo})):result.nodes.map(n=>({...n,demo:result.preview&&n.online}));hooks.setData(publicNodes,site,result.preview,result.themeRevision);renewalUI.update(records);if(asAdmin)void operations.pollVersions();if(adminOpen&&!draggingNode){if(background&&activeTab==='servers')serverUI.update();else if(!background||activeTab==='overview')renderAdmin();if(activeTab==='versions')operations.update();}}catch(error){if(error.staleSession)return;hooks.onLoadError(error.message);if(error.status!==403&&error.status!==401)toast(error.message,true);}finally{loading=false;if(refreshQueued){refreshQueued=false;void refresh();}}}
 function openLogin(){if(auth.authenticated){openAdmin();return;}$('#login-error').textContent='';$('#login-dialog').showModal();$('#login-form [name="username"]').focus();api('/api/session').then(v=>{auth=v;csrf=v.csrf;authUI();}).catch(e=>$('#login-error').textContent=e.message);}
 $('#login-dialog').addEventListener('close',()=>{$('#login-form [name="password"]').value='';$('#login-form').elements.code.value='';});
 $('#login-form').addEventListener('submit',async event=>{event.preventDefault();const f=event.currentTarget,b=f.querySelector('button[type="submit"]');b.disabled=true;$('#login-error').textContent='';try{auth=await api('/api/login','POST',{username:f.elements.username.value.trim(),password:f.elements.password.value,code:f.elements.code.value});csrf=auth.csrf;authUI();closeDialog('#login-dialog');await refresh();openAdmin();toast('管理员登录成功');}catch(error){$('#login-error').textContent=error.message;f.elements.password.value='';}finally{b.disabled=false;}});
 $('#admin-entry').addEventListener('click',()=>auth.authenticated?openAdmin():openLogin());$('#admin-badge').addEventListener('click',()=>openAdmin());$('.login-symbol').replaceChildren(icon('shield'));
 function button(label,cls,action,glyph){const b=el('button',cls);b.type='button';if(glyph)b.append(icon(glyph));b.append(document.createTextNode(label));b.addEventListener('click',action);return b;}
 function openAdmin(tab='overview'){if(!auth.authenticated){openLogin();return;}activeTab=tab;adminOpen=true;$('#main-content').hidden=true;$('#admin-view').hidden=false;renderAdmin();window.scrollTo({top:0});}
 function closeAdmin(){adminOpen=false;$('#admin-view').hidden=true;$('#main-content').hidden=false;hooks.onBoardOpen?.();window.scrollTo({top:0});}
 $('.brand').addEventListener('click',event=>{if(adminOpen){event.preventDefault();closeAdmin();}});
 function status(record){const n=record.public;const s=el('span','status-badge'+(n.online?'':' is-offline'));s.append(el('i','status-dot '+(n.online?'online':'offline')),document.createTextNode(n.pending?'待接入':n.online?'在线':'离线'));return s;}
 function nodeName(record){const wrap=el('div','admin-node-name');const f=el('span','flag-wrap');f.append(flag(record.public.code));const name=el('div');name.append(el('strong','',record.public.name),el('small','',record.public.country));wrap.append(f,name);return wrap;}
 function nodeTable(compact=false,rows=records,{ordering=true,health=false,emptyMessage='还没有服务器，添加第一台开始监控。'}={}){const wrap=el('div','table-scroll');const table=el('table','node-table');const thead=el('thead'),head=el('tr');for(const label of compact?['服务器','SSH 地址','状态','操作']:['服务器','SSH 地址','状态',...(health?['连接信息']:[]),'公开展示','操作'])head.append(el('th','',label));thead.append(head);const body=el('tbody');for(const r of [...rows].sort((a,b)=>(a.public.order||0)-(b.public.order||0))){const tr=el('tr');tr.dataset.id=r.public.id;const name=el('td');name.append(nodeName(r));if(r.public.group||r.public.pinned)name.append(el('small','node-group',(r.public.pinned?'置顶 · ':'')+(r.public.group||'未分组')));if(!compact&&r.providerName)name.append(el('small','admin-node-provider',r.providerName));if(!compact&&ordering)addOrdering(tr,name,r);const addr=el('td','address-cell');addr.append(el('code','',r.ip),el('small','',r.username+' · '+r.port));if(r.resolvedIPs)addr.append(el('small','',r.resolvedIPs.length?'解析 IP：'+r.resolvedIPs.join('、'):'等待域名解析'));if(r.agentPeerIP)addr.append(el('small','','接入 IP：'+r.agentPeerIP));else if(r.detectedIP)addr.append(el('small','','上报 IP：'+r.detectedIP));const state=el('td');state.append(status(r));if(r.expiresAt)state.append(el('small','agent-version'+(r.renewalDue?' expiry-due':''),'到期 '+formatExpiry(r.expiresAt)));if(r.agentVersion)state.append(el('small','agent-version','Agent '+r.agentVersion));if(r.deployMessage&&r.deployState!=='done')state.append(el('small','deploy-status '+r.deployState,r.deployMessage));tr.append(name,addr,state);if(health){const info=el('td');info.append(serverUI.health(r));tr.append(info);}if(!compact)tr.append(el('td','',r.visible?'公开':'仅管理员'));const ops=el('td');const buttons=el('div','row-actions');if(!r.removing)buttons.append(button('SSH','table-button',()=>openTerminal(r.public.id),'terminal'),button('运行状态','table-button',()=>inspectorUI.open(r)));if(!compact){buttons.append(button('编辑','table-button',()=>openEditor(r.public.id),'edit'),button(r.management?.action==='remove'?'重新检查移除':'移除','table-button danger-text',()=>confirmDelete(r.public.id),'trash'));}if(!compact&&r.management?.action==='remove'&&['failed','retry'].includes(r.management.state)){buttons.append(button('仅移除记录','table-button danger-text',()=>confirmDelete(r.public.id,true)));}ops.append(buttons);tr.append(ops);body.append(tr);}table.append(thead,body);wrap.append(table);if(!rows.length)wrap.append(el('p','admin-empty',emptyMessage));return wrap;}

 async function reorder(id,target,direction=0){const ids=[...records].sort((a,b)=>(a.public.order||0)-(b.public.order||0)).map(r=>r.public.id);const from=ids.indexOf(id),to=direction?from+direction:ids.indexOf(target);if(from<0||to<0||to>=ids.length||from===to)return;ids.splice(from,1);ids.splice(to,0,id);try{await api('/api/admin/node-order','POST',{ids});await refresh();toast('顺序已保存');}catch(e){toast(e.message,true);}}
 function addOrdering(tr,cell,r){const controls=el('div','node-ordering');for(const [text,dir]of [['↑',-1],['↓',1]]){const b=button(text,'table-button',()=>reorder(r.public.id,null,dir));b.setAttribute('aria-label',(dir<0?'上移 ':'下移 ')+r.public.name);controls.append(b);}cell.append(controls);tr.draggable=true;tr.addEventListener('dragstart',e=>{draggingNode=r.public.id;e.dataTransfer.setData('text/plain',draggingNode);e.dataTransfer.effectAllowed='move';tr.classList.add('is-dragging');});tr.addEventListener('dragend',()=>{draggingNode=null;tr.classList.remove('is-dragging');});tr.addEventListener('dragover',e=>{if(draggingNode){e.preventDefault();e.dataTransfer.dropEffect='move';}});tr.addEventListener('drop',e=>{e.preventDefault();const id=draggingNode;draggingNode=null;if(id)reorder(id,r.public.id);});}
 function sectionHeading(title,description){const h=el('div','admin-section-heading');const text=el('div');text.append(el('h2','',title));if(description)text.append(el('p','',description));h.append(text);return h;}
 function overview(panel){const stats=el('div','admin-stats');for(const [label,value,glyph]of [['服务器',records.length,'server'],['在线',records.filter(n=>n.public.online).length,'activity'],['待接入',records.filter(n=>n.public.pending).length,'plus'],['离线',records.filter(n=>!n.public.online&&!n.public.pending).length,'clock']]){const item=el('div','glass stat-tile');const top=el('span');top.append(icon(glyph),document.createTextNode(label));item.append(top,el('strong','',value));stats.append(item);}panel.append(stats);const box=el('section','glass admin-box');const heading=sectionHeading('节点概况','管理员可见 IP 与 SSH 入口');heading.append(button('添加服务器','primary-button',()=>openEditor(),'plus'));box.append(heading,nodeTable(true,records.slice(0,20)));if(records.length>20)box.append(button('查看全部 '+records.length+' 台','secondary-button',()=>openAdmin('servers')));panel.append(box);const note=el('section','glass connection-note');note.append(icon('shield'),el('div','','Agent 通过独立 WSS 通道上报。浏览器终端须经过管理员验证与节点授权。'));panel.append(note);}
 function servers(panel){serverUI.render(panel);}
 function field(label,type,name,value){const wrap=el('label','form-field',label);const input=el('input');input.type=type;input.name=name;input.value=String(value??'');wrap.append(input);return [wrap,input];}
 async function commandSettings(panel){const box=el('section','glass admin-box');const heading=sectionHeading('常用命令','在 SSH 终端中快速调用');heading.append(button('添加命令','primary-button',()=>editCommand(),'plus'));box.append(heading);const list=el('div','command-list');box.append(list);panel.append(box);try{commands=(await api('/api/admin/commands')).commands;if(!box.isConnected)return;for(const command of commands){const row=el('article','command-item');const info=el('div','command-info');info.append(el('strong','',command.name),el('pre','',command.script));const actions=el('div','row-actions');actions.append(button('编辑','table-button',()=>editCommand(command.id),'edit'),button('删除','table-button danger-text',async()=>{await removeCommand(command.id);},'trash'));row.append(info,actions);list.append(row);}if(!commands.length)list.append(el('p','admin-empty','还没有常用命令，添加后即可在终端中使用。'));}catch(e){list.textContent=e.message;}}
 function editCommand(id){const c=commands.find(c=>c.id===id),form=$('#command-form');form.reset();form.elements.id.value=id||'';form.elements.name.value=c?.name||'';form.elements.script.value=c?.script||'';$('#command-error').textContent='';$('#command-editor-title').textContent=c?'编辑常用命令':'添加常用命令';$('#command-editor').showModal();form.elements.name.focus();}
 async function removeCommand(id){try{await api('/api/admin/commands/'+encodeURIComponent(id),'DELETE');renderAdmin();toast('命令已删除');}catch(e){toast(e.message,true);}}
 $('#command-form').addEventListener('submit',async event=>{event.preventDefault();const f=event.currentTarget,b=f.querySelector('button[type="submit"]');b.disabled=true;$('#command-error').textContent='';try{const id=f.elements.id.value;await api('/api/admin/commands'+(id?'/'+encodeURIComponent(id):''),id?'PATCH':'POST',{name:f.elements.name.value,script:f.elements.script.value});closeDialog('#command-editor');renderAdmin();toast('常用命令已保存');}catch(e){$('#command-error').textContent=e.message;}finally{b.disabled=false;}});
 function siteSettings(panel){const box=el('section','glass admin-box settings-box');box.append(sectionHeading('面板设置','管理公开看板与显示选项'));const form=el('form','settings-form');const [name,nameInput]=field('站点名称','text','name',site.name);nameInput.required=true;nameInput.maxLength=60;const [refreshField,refreshInput]=field('刷新间隔（秒）','number','refreshSeconds',site.refreshSeconds);refreshInput.min='3';refreshInput.max='30';refreshInput.required=true;const check=el('label','check-field');const input=el('input');input.type='checkbox';input.name='public';input.checked=site.public;check.append(input,document.createTextNode('允许访客查看服务器看板'));form.append(name,refreshField,check,el('p','form-footnote','IP、SSH 信息与管理操作始终仅管理员可见。'));const save=el('button','primary-button','保存设置');save.type='submit';form.append(save);form.addEventListener('submit',async e=>{e.preventDefault();save.disabled=true;try{const result=await api('/api/admin/site','PUT',{name:nameInput.value.trim(),refreshSeconds:Number(refreshInput.value),public:input.checked});siteEpoch++;site=result.site;hooks.setSite?.(site);await refresh();toast('面板设置已保存');}catch(e){toast(e.message,true);}finally{save.disabled=false;}});box.append(form);panel.append(box);mfaSettings(panel);}
 async function notificationSettings(panel){
  const box=el('section','glass admin-box settings-box telegram-settings');
  box.append(sectionHeading('Telegram 通知','选择提醒事件，及时了解服务器状态'));
  const loading=el('p','form-footnote','正在读取设置…');box.append(loading);panel.append(box);
  try{
   let [saved,previewData]=await Promise.all([api('/api/admin/telegram'),api('/api/admin/telegram/preview')]);
   if(!box.isConnected)return;
   loading.remove();
   const layout=el('div','telegram-layout'),form=el('form','settings-form');form.autocomplete='off';
   const preview=el('aside','telegram-preview'),previewTitle=el('h3','','消息预览'),previewNote=el('p','form-footnote','示例消息，不会实际发送。'),previewSelect=el('select','telegram-preview-select');
   previewSelect.setAttribute('aria-label','通知预览类型');
   const bubble=el('div','telegram-message'),openPanel=el('div','telegram-open-panel','打开面板 ↗');
   // Recreate only the three formatting tags we use; never insert server HTML into the page.
   function renderMessage(markup){
    const doc=new DOMParser().parseFromString(markup,'text/html'),fragment=document.createDocumentFragment();
    function append(parent,node){
     if(node.nodeType===Node.TEXT_NODE){parent.append(document.createTextNode(node.textContent));return;}
     if(node.nodeType!==Node.ELEMENT_NODE)return;
     if(['B','I','CODE'].includes(node.tagName)){const child=document.createElement(node.tagName.toLowerCase());parent.append(child);for(const item of node.childNodes)append(child,item);}
     else parent.append(document.createTextNode(node.textContent));
    }
    for(const node of doc.body.childNodes)append(fragment,node);
    bubble.replaceChildren(fragment);
   }
   for(const item of previewData.previews){const option=el('option','',item.label);option.value=item.kind;previewSelect.append(option);}
   const showPreview=()=>renderMessage(previewData.previews.find(v=>v.kind===previewSelect.value)?.html||'');
   previewSelect.addEventListener('change',showPreview);showPreview();
   preview.append(previewTitle,previewNote,previewSelect,bubble,openPanel);
   const settingsColumn=el("div","telegram-settings-column");settingsColumn.append(form);layout.append(settingsColumn,preview);box.append(layout);
   const check=(label,name,checked)=>{const wrap=el('label','check-field'),input=el('input');input.type='checkbox';input.name=name;input.checked=checked;wrap.append(input,document.createTextNode(label));return [wrap,input];};
   const [enabledField,enabled]=check('启用接收人通知','enabled',saved.enabled);
   const [tokenField,tokenInput]=field('Bot Token','password','telegramToken','');
   tokenInput.autocomplete='new-password';tokenInput.spellcheck=false;tokenInput.maxLength=160;
   const [chatField,chat]=field('接收人 ID','text','chatId',saved.chatId);
   chat.placeholder='个人或群组的 Chat ID';chat.maxLength=17;chat.autocomplete='off';chat.spellcheck=false;
   const events=el('fieldset','notification-events');events.append(el('legend','','通知事件'));
   const [onlineField,online]=check('服务器上线','notifyOnline',saved.notifyOnline);
   const [offlineField,offline]=check('服务器离线','notifyOffline',saved.notifyOffline);
   const [renewalField,renewal]=check('服务器续费','notifyRenewal',saved.notifyRenewal);
   const [loginField,login]=check('管理员登录','notifyLogin',saved.notifyLogin);events.append(onlineField,offlineField,renewalField,loginField);
   const [clearField,clear]=check('清除已保存的 Token','clearToken',false);
   const note=el('p','form-footnote','请先向 Bot 发送 /start。离线持续 20 秒后提醒；续费提前 3 天提醒，同日到期自动合并，每个周期发送一次（UTC+8）。');
   const error=el('p','form-error');error.setAttribute('role','alert');
   const statusBox=el('div','notification-status');statusBox.setAttribute('role','status');statusBox.setAttribute('aria-live','polite');
   const actions=el('div','notification-actions');
   const save=el('button','primary-button','保存设置');save.type='submit';
   const test=el('button','secondary-button','发送测试通知');test.type='button';
   const refreshStatus=button('刷新状态','table-button',async()=>{try{updateStatus(await api('/api/admin/telegram'));}catch(e){error.textContent=e.message;}});
   actions.append(save,test);
   let dirty=false,busy=false,updateGroupStatus=()=>{};
   function updateStatus(v){
    saved=v;updateGroupStatus(v.group||{});
    const lines=[];
    if(v.pending)lines.push('待发送：'+v.pending+' 条');
    if(v.test?.status==='pending')lines.push('测试通知正在发送…');
    if(v.test?.status==='sent')lines.push('测试通知已发送');
    if(v.test?.status==='failed')lines.push('测试失败：'+v.test.error);
    if(v.lastSuccess)lines.push('最近发送：'+new Date(v.lastSuccess*1000).toLocaleString('zh-CN',{hour12:false}));
    if(v.lastError)lines.push(v.lastError);
    if(v.dropped)lines.push('队列满时未发送的事件：'+v.dropped+' 条');
    if(!lines.length)lines.push(v.hasToken?'尚未发送通知':'尚未配置');
    statusBox.replaceChildren(...lines.map(text=>el('p','',text)),refreshStatus);
    test.disabled=busy||dirty||!v.hasToken||!v.chatId||v.test?.status==='pending';
   }
   function reflectSaved(v){
    tokenInput.value='';tokenInput.placeholder=v.hasToken?'已保存，留空则保留':'填写 BotFather 提供的 Token';
    clear.checked=false;clearField.hidden=!v.hasToken;dirty=false;updateStatus(v);
   }
   form.addEventListener('input',()=>{dirty=true;test.disabled=true;error.textContent='';});
   clear.addEventListener('change',()=>{if(clear.checked){tokenInput.value='';enabled.checked=false;}});
   form.addEventListener('submit',async e=>{
    e.preventDefault();busy=true;save.disabled=true;test.disabled=true;error.textContent='';
    try{
     const v=await api('/api/admin/telegram','PUT',{enabled:enabled.checked,token:tokenInput.value.trim(),chatId:chat.value.trim(),notifyOnline:online.checked,notifyOffline:offline.checked,notifyRenewal:renewal.checked,notifyLogin:login.checked,clearToken:clear.checked});
     if(!box.isConnected)return;
     reflectSaved(v);toast('Telegram 通知设置已保存');
    }catch(e){error.textContent=e.message;}
    finally{busy=false;save.disabled=false;updateStatus(saved);}
   });
   test.addEventListener('click',async()=>{
    busy=true;test.disabled=true;error.textContent='';
    try{await api('/api/admin/telegram/test','POST',{});updateStatus(await api('/api/admin/telegram'));toast('测试通知已提交');}
    catch(e){error.textContent=e.message;}
    finally{busy=false;updateStatus(saved);}
   });
   form.append(enabledField,tokenField,chatField,events,clearField,note,error,actions,statusBox);
   const groupForm=el('form','settings-form telegram-group-form');groupForm.autocomplete='off';
   const [groupEnabledField,groupEnabled]=check('启用群组通知','groupEnabled',saved.group?.enabled===true);
   const [groupChatField,groupChat]=field('群组 ID','text','groupChatId',saved.group?.chatId||'');
   groupChat.placeholder='例如 -1001234567890';groupChat.maxLength=17;groupChat.spellcheck=false;
   const groupNote=el('p','form-footnote','使用上方保存的 Bot。请先将 Bot 加入群组并设为管理员；本群仅接收服务器上线、离线和恢复通知。');
   const groupError=el('p','form-error'),groupStatus=el('div','notification-status'),groupActions=el('div','notification-actions');
   groupError.setAttribute('role','alert');groupStatus.setAttribute('role','status');
   const groupSave=el('button','primary-button','验证并保存'),groupTest=el('button','secondary-button','发送群组测试');
   groupSave.type='submit';groupTest.type='button';groupActions.append(groupSave,groupTest);
   groupForm.append(el('h3','','群组通知'),groupEnabledField,groupChatField,groupNote,groupError,groupActions,groupStatus);settingsColumn.append(groupForm);
   let groupDirty=false,groupBusy=false;
   updateGroupStatus=g=>{
    if(!groupDirty){groupEnabled.checked=g.enabled===true;groupChat.value=g.chatId||'';}
    const lines=[];
    if(g.enabled)lines.push((g.title||'群组')+' · 已启用');
    if(g.verifiedAt)lines.push('已确认机器人管理员权限');
    if(g.test?.status==='pending')lines.push('群组测试正在发送…');
    if(g.test?.status==='sent')lines.push('群组测试已发送');
    if(g.test?.status==='failed')lines.push('群组测试失败：'+g.test.error);
    if(g.lastSuccess)lines.push('最近发送：'+new Date(g.lastSuccess*1000).toLocaleString('zh-CN',{hour12:false}));
    if(g.lastError)lines.push(g.lastError);
    if(!lines.length)lines.push('尚未启用群组通知');
    groupStatus.replaceChildren(...lines.map(text=>el('p','',text)));
    groupTest.disabled=groupBusy||groupDirty||dirty||!g.enabled||g.test?.status==='pending';
    groupSave.disabled=groupBusy||dirty||busy;
   };
   groupForm.oninput=()=>{groupDirty=true;groupError.textContent='';updateGroupStatus(saved.group||{});};
   form.addEventListener('input',()=>updateGroupStatus(saved.group||{}));
   groupForm.onsubmit=async e=>{
    e.preventDefault();if(dirty||busy)return;groupBusy=true;groupError.textContent='';updateGroupStatus(saved.group||{});
    try{const v=await api('/api/admin/telegram/group','PUT',{enabled:groupEnabled.checked,chatId:groupChat.value.trim()});groupDirty=false;updateStatus(v);toast('群组通知设置已保存');}
    catch(e){groupError.textContent=e.message;}
    finally{groupBusy=false;updateGroupStatus(saved.group||{});}
   };
   groupTest.onclick=async()=>{
    groupBusy=true;groupError.textContent='';updateGroupStatus(saved.group||{});
    try{await api('/api/admin/telegram/group/test','POST',{});updateStatus(await api('/api/admin/telegram'));toast('群组测试已提交');}
    catch(e){groupError.textContent=e.message;}
    finally{groupBusy=false;updateGroupStatus(saved.group||{});}
   };
   reflectSaved(saved);
   async function poll(){
    if(!box.isConnected||!adminOpen||activeTab!=='notifications'||!auth.authenticated)return;
    try{const v=await api('/api/admin/telegram');if(box.isConnected)updateStatus(v);}catch(e){if(!e.staleSession&&box.isConnected)error.textContent=e.message;}
    if(box.isConnected&&adminOpen&&activeTab==='notifications'&&auth.authenticated)setTimeout(poll,5000);
   }
   setTimeout(poll,5000);
  }catch(e){if(box.isConnected){loading.textContent=e.message;if(!loading.isConnected)box.append(loading);}}
 }
 function usernameSettings(panel){
  const box=el('section','glass admin-box settings-box');
  box.append(sectionHeading('管理员用户名','当前用户名：'+auth.username));
  const form=el('form','settings-form');
  const [name,nameInput]=field('新用户名','text','username',auth.username);
  nameInput.required=true;nameInput.maxLength=32;nameInput.pattern='[A-Za-z_][A-Za-z0-9_.\\-]{0,31}';nameInput.autocomplete='username';nameInput.autocapitalize='none';nameInput.spellcheck=false;
  const [password,passwordInput]=field('当前密码','password','current','');
  passwordInput.required=true;passwordInput.maxLength=72;passwordInput.autocomplete='current-password';
  const error=el('p','form-error');error.setAttribute('role','alert');
  const save=el('button','primary-button','修改用户名');save.type='submit';
  form.append(name,password,el('p','form-footnote','支持 1–32 位字母、数字、下划线、点和连字符，以字母或下划线开头。保存后其他登录将退出，已打开的终端需重新连接。'),error,save);
  form.addEventListener('submit',async event=>{
   event.preventDefault();save.disabled=true;error.textContent='';
   try{
    auth=await api('/api/admin/username','POST',{username:nameInput.value.trim(),current:passwordInput.value});csrf=auth.csrf;
    await terminalUI.close(true);authUI();renderAdmin();toast('用户名已更新，其他登录已退出');
   }catch(e){error.textContent=e.message;}
   finally{passwordInput.value='';save.disabled=false;}
  });
  box.append(form);panel.append(box);
 }
 function security(panel){usernameSettings(panel);const box=el('section','glass admin-box settings-box');box.append(sectionHeading('账户安全','当前管理员：'+auth.username));const status=el('div','security-summary');for(const label of ['服务端密码验证','HttpOnly 安全会话','CSRF 请求校验']){const item=el('span');item.append(icon('check'),document.createTextNode(label));status.append(item);}box.append(status);const form=el('form','settings-form');const [current,currentInput]=field('当前密码','password','current','');currentInput.autocomplete='current-password';currentInput.required=true;const [next,nextInput]=field('新密码','password','new','');nextInput.autocomplete='new-password';nextInput.minLength=12;nextInput.maxLength=72;nextInput.required=true;const [confirm,confirmInput]=field('确认新密码','password','confirm','');confirmInput.autocomplete='new-password';confirmInput.required=true;const error=el('p','form-error');error.setAttribute('role','alert');const save=el('button','primary-button','更新密码');save.type='submit';form.append(current,next,confirm,el('p','form-footnote','更新密码后，其他管理会话会退出登录。'),error,save);form.addEventListener('submit',async event=>{event.preventDefault();error.textContent='';if(nextInput.value!==confirmInput.value){error.textContent='两次填写的新密码不一致';return;}save.disabled=true;try{auth=await api('/api/admin/password','POST',{current:currentInput.value,new:nextInput.value});csrf=auth.csrf;form.reset();toast('密码已更新，其他会话已退出');}catch(e){error.textContent=e.message;}finally{save.disabled=false;}});box.append(form);panel.append(box);}
 function mfaSettings(panel){const box=el('section','glass admin-box settings-box');box.append(sectionHeading('二步验证',auth.mfaEnabled?'已启用 · 登录需要验证器动态验证码':'绑定身份验证器，为管理入口增加一层保护'));const form=el('form','settings-form');const [password,passwordInput]=field('当前密码','password','password','');passwordInput.required=true;passwordInput.autocomplete='current-password';const [code,codeInput]=field('验证器验证码','text','code','');codeInput.inputMode='text';codeInput.pattern=auth.mfaEnabled?'[0-9]{6}|[a-fA-F0-9-]{32,35}':'[0-9]{6}';codeInput.maxLength=auth.mfaEnabled?35:6;codeInput.autocomplete='one-time-code';code.hidden=!auth.mfaEnabled;codeInput.required=!!auth.mfaEnabled;const secret=el('div','mfa-secret');secret.hidden=true;const error=el('p','form-error');error.setAttribute('role','alert');const save=el('button','primary-button',auth.mfaEnabled?'关闭二步验证':'开始绑定');save.type='submit';let preparing=false;form.append(password,secret,code,error,save,el('p','form-footnote','遗失验证器时，可填入未使用的恢复码验证并关闭二步验证。'));form.addEventListener('submit',async e=>{e.preventDefault();save.disabled=true;error.textContent='';try{if(!auth.mfaEnabled&&!preparing){const result=await api('/api/admin/mfa/setup','POST',{password:passwordInput.value});secret.replaceChildren(el('p','','使用身份验证器扫码，或手动填写密钥：'),authenticatorCanvas(result.secret,site.name,auth.username),el('code','',result.secret),el('small','','账户：'+auth.username+' · 6 位数字 · 30 秒'));secret.hidden=false;code.hidden=false;codeInput.required=true;password.hidden=true;passwordInput.required=false;passwordInput.value='';preparing=true;save.textContent='验证并启用';codeInput.focus();}else{auth=await api('/api/admin/mfa/'+(auth.mfaEnabled?'disable':'enable'),'POST',{password:passwordInput.value,code:codeInput.value});csrf=auth.csrf;authUI();renderAdmin();if(auth.recoveryCodes?.length)operations.showRecovery(auth.recoveryCodes);toast(auth.mfaEnabled?'二步验证已启用':'二步验证已关闭');}}catch(e){error.textContent=e.message;}finally{save.disabled=false;passwordInput.value='';}});box.append(form);panel.append(box);}
 async function audit(panel){const box=el('section','glass admin-box');box.append(sectionHeading('操作记录','记录登录、节点管理与设置变更'));const list=el('div','audit-list');box.append(list);panel.append(box);try{const result=await api('/api/admin/audit');if(!box.isConnected)return;for(const event of [...result.events].reverse()){const row=el('div','audit-row');row.append(el('time','',new Date(event.at).toLocaleString('zh-CN',{hour12:false})),el('strong','',labels[event.action]||event.action),el('span','',event.subject));list.append(row);}if(!result.events.length)list.append(el('p','admin-empty','暂无操作记录'));}catch(e){list.textContent=e.message;}}
 function renderAdmin(){if(!auth.authenticated)return;const focusedStatus=document.activeElement?.dataset.serverStatus;endAppearancePreview();const root=$('#admin-view');const top=el('div','admin-page-heading');const titles=el('div');titles.append(el('div','eyebrow','ADMINISTRATION'),el('h1','','管理后台'));const actions=el('div','admin-heading-actions');actions.append(button('返回看板','secondary-button',closeAdmin,'arrow'),button('退出登录','secondary-button',logout,'logout'));const versionNotice=button('','secondary-button version-notice',()=>openAdmin('versions'));versionNotice.dataset.versionNotice='';versionNotice.hidden=true;actions.prepend(versionNotice);top.append(titles,actions);const layout=el('div','admin-layout');const sidebar=el('nav','glass admin-nav');sidebar.setAttribute('aria-label','管理菜单');for(const [id,label,glyph]of [['overview','概览','grid'],['servers','服务器','server'],['billing','支出汇总','clock'],['commands','常用命令','terminal'],['settings','面板设置','settings'],['themes','主题','grid'],['notifications','Telegram 通知','activity'],['backups','备份迁移','server'],['versions','版本管理','activity'],['security','账户安全','shield'],['audit','操作记录','clock']]){const b=button(label,'admin-nav-item'+(activeTab===id?' active':''),()=>{activeTab=id;renderAdmin();},glyph);b.dataset.tab=id;if(activeTab===id)b.setAttribute('aria-current','page');sidebar.append(b);}const content=el('div','admin-content');layout.append(sidebar,content);root.replaceChildren(top,layout);operations.showNotice();({overview,servers,billing:panel=>showBilling({panel,el,api,records,edit:openEditor,heading:sectionHeading}),themes:panel=>showThemes({panel,el,api,heading:sectionHeading,toast}),commands:commandSettings,settings:siteSettings,notifications:notificationSettings,backups:operations.backups,versions:operations.versions,security:(panel)=>{security(panel);operations.sessions(panel);},audit}[activeTab]||overview)(content);if(activeTab==='servers'&&focusedStatus)Array.from(root.querySelectorAll('[data-server-status]')).find(b=>b.dataset.serverStatus===focusedStatus)?.focus({preventScroll:true});}
 const accessMethod=el('select');accessMethod.name='accessMethod';accessMethod.append(new Option('SSH 密码自动安装','ssh'),new Option('一次性命令接入','command'));const accessLabel=el('label','form-field','接入方式');accessLabel.append(accessMethod);$('#node-form').elements.password.closest('label').before(accessLabel);
 accessMethod.onchange=()=>{const f=$('#node-form'),manual=accessMethod.value==='command';f.elements.password.closest('label').hidden=manual;f.elements.password.required=!manual&&!f.elements.id.value;$('#node-save').textContent=manual?'保存并生成命令':f.elements.id.value?'保存服务器':'安装并接入';$('#deployment-note').textContent=manual?'在目标服务器以 root 或 sudo 执行命令，10 分钟内有效，只能使用一次。':'填写 SSH 信息后自动安装探针。密码仅用于本次安装。';};
 const enrollmentDialog=el('dialog','node-dialog editor-dialog'),enrollmentBody=el('div','dialog-inner'),enrollmentTitle=el('h2','','一次性接入命令'),enrollmentText=el('textarea'),enrollmentInfo=el('p','form-footnote'),enrollmentActions=el('div','dialog-actions');enrollmentText.readOnly=true;enrollmentText.rows=5;enrollmentText.setAttribute('aria-label','一次性接入命令');let enrollmentNode='';
 const enrollClose=button('关闭','secondary-button',()=>enrollmentDialog.close()),enrollCopy=button('复制命令','primary-button',async()=>{try{await navigator.clipboard.writeText(enrollmentText.value);toast('已复制');}catch{enrollmentText.select();toast('请长按复制命令');}}),enrollRevoke=button('撤销命令','secondary-button',async()=>{try{await api('/api/admin/enrollment','POST',{id:enrollmentNode,revoke:true});enrollmentDialog.close();toast('接入命令已撤销');}catch(e){toast(e.message,true);}});
 enrollmentActions.append(enrollRevoke,enrollClose,enrollCopy);enrollmentBody.append(enrollmentTitle,enrollmentInfo,enrollmentText,enrollmentActions);enrollmentDialog.append(enrollmentBody);document.body.append(enrollmentDialog);enrollmentDialog.addEventListener('close',()=>{enrollmentText.value='';enrollmentNode='';});
 async function showEnrollment(id){const r=await api('/api/admin/enrollment','POST',{id});enrollmentNode=id;enrollmentText.value=r.command;enrollmentInfo.textContent='在目标服务器执行。有效至 '+new Date(r.expires*1000).toLocaleTimeString('zh-CN',{hour12:false})+'；使用后立即失效。';enrollmentDialog.showModal();}
 const fillLease=setupLeaseFields($('#node-form'));
 function openEditor(id){if(!auth.authenticated)return;const r=records.find(n=>n.public.id===id);const form=$('#node-form');form.reset();fillLease(r);form.elements.id.value=id||'';form.elements.name.value=r?.public.name||'';form.elements.group.value=r?.public.group||'';form.elements.pinned.checked=!!r?.public.pinned;form.elements.notes.value=r?.notes||'';$('#node-groups').replaceChildren(...[...new Set(records.map(n=>n.public.group).filter(Boolean))].map(g=>new Option(g,g)));form.elements.ip.value=r?.ip||'';form.elements.port.value=r?.port||22;form.elements.username.value=r?.username||'root';form.elements.password.value='';form.elements.code.value=r?.countryAuto?'OTHER':r?.public.code||'OTHER';form.elements.password.required=!r;form.elements.password.placeholder=r?'留空保留，填写重新部署':'仅用于安装 Agent';$('#node-save').textContent=r?'保存服务器':'安装并接入';form.elements.city.value=r?.public.city==='待识别'?'':r?.public.city||'';form.elements.visible.checked=r?r.visible:true;form.elements.providerName.value=r?.providerName||'';form.elements.providerURL.value=r?.providerURL||'';form.elements.expiresAt.value=localExpiry(r?.expiresAt);form.elements.notifyRenewal.checked=!!r?.notifyRenewal;form.elements.renewalVersion.value=r?.renewalVersion||'';$('#editor-title').textContent=r?'编辑服务器':'添加服务器';$('#node-error').textContent='';accessMethod.value='ssh';accessMethod.onchange();$('#node-editor').showModal();form.elements.name.focus();}
 $('#node-editor').addEventListener('close',()=>$('#node-form').elements.password.value='');
 async function watchDeployment(id){try{for(let i=0;i<70;i++){await new Promise(r=>setTimeout(r,4000));if(!auth.authenticated)return;const {job}=await api('/api/admin/deploy/'+encodeURIComponent(id));await refresh();if(job.state!=='running'){toast(job.message,job.state!=='done');return;}}}catch(e){toast(e.message,true);}}
 $('#node-form').addEventListener('submit',async event=>{event.preventDefault();const f=event.currentTarget,id=f.elements.id.value,save=$('#node-save');save.disabled=true;$('#node-error').textContent='';let password=accessMethod.value==='command'?'':f.elements.password.value;const manual=accessMethod.value==='command';try{const code=f.elements.code.value;
 const result=await api('/api/admin/nodes'+(id?'/'+encodeURIComponent(id):''),id?'PATCH':'POST',{name:f.elements.name.value.trim(),group:f.elements.group.value.trim(),pinned:f.elements.pinned.checked,notes:f.elements.notes.value,ip:f.elements.ip.value.trim(),port:Number(f.elements.port.value),username:f.elements.username.value.trim(),password:'',country:countries[code]||code,city:f.elements.city.value.trim(),code,visible:f.elements.visible.checked,providerName:f.elements.providerName.value.trim(),providerURL:f.elements.providerURL.value.trim(),expiresAt:f.elements.expiresAt.value?(f.elements.expiresAt.value===localExpiry(records.find(r=>r.public.id===id)?.expiresAt)?records.find(r=>r.public.id===id).expiresAt:new Date(f.elements.expiresAt.value).toISOString()):'',notifyRenewal:f.elements.notifyRenewal.checked,renewalVersion:f.elements.renewalVersion.value,...leaseInput(f)});f.elements.id.value=result.node.public.id;
 if(password){const deployment=await api('/api/admin/deploy','POST',{id:result.node.public.id,password});watchDeployment(deployment.job.id);toast('部署已开始，可在服务器列表查看进度。');}else if(!manual)toast('服务器已保存');closeDialog('#node-editor');await refresh();if(manual)await showEnrollment(result.node.public.id);}catch(e){$('#node-error').textContent=e.message;}finally{password='';f.elements.password.value='';save.disabled=false;$('#deployment-note').textContent='填写 SSH 信息后自动安装探针。密码仅用于本次安装。';}});
 function confirmDelete(id,forget=false){forgetRemoval=forget;const record=records.find(r=>r.public.id===id);if(!record)return;removeId=id;$('#delete-description').textContent=forget?'仅撤销面板中的记录与凭据？远端 Agent 需要在被控机上手动清理。':record.public.online?'移除 '+record.public.name+' 并清除其 Agent 配置？其他应用不受影响。':'该节点已离线，仅移除面板记录并撤销凭据。服务器上的其他应用不会被修改。';$('#delete-dialog').showModal();}
 $('#delete-cancel').addEventListener('click',()=>closeDialog('#delete-dialog'));$('#delete-confirm').addEventListener('click',async()=>{const b=$('#delete-confirm');b.disabled=true;try{const removal=await api('/api/admin/nodes/'+encodeURIComponent(removeId),'DELETE',forgetRemoval?{forget:true}:undefined);closeDialog('#delete-dialog');await refresh();toast(removal.pending?'正在清除远端探针配置':'服务器记录已移除');}catch(e){toast(e.message,true);}finally{b.disabled=false;}});
 async function logout(){try{const result=await api('/api/logout','POST',{});terminalUI.close(true);loseAuth();auth=result;csrf=result.csrf||'';await refresh();toast('已退出管理员登录');}catch(e){toast(e.message,true);}}
 const terminalUI=createTerminalUI(api,()=>records,toast);
 const inspectorUI=createInspectorUI(api);
 const serverUI=createServerManagement({el,button,heading:sectionHeading,getRecords:()=>records,nodeTable,openEditor});
 const operations=createOperations({el,api,toast,field,button,heading:sectionHeading,getRecords:()=>records,getAuth:()=>auth,refresh});
 function openTerminal(id){const record=records.find(n=>n.public.id===id);if(auth.authenticated&&record)terminalUI.open(record);}
 function decorateCard(card,node){if(!auth.authenticated)return;const record=records.find(n=>n.public.id===node.id);if(!record)return;const line=el('div','admin-ip-row');const ip=el('span');ip.append(icon('shield'),el('code','',record.currentIP));ip.title=record.ip;line.append(ip,button('SSH','ssh-button',()=>openTerminal(node.id),'terminal'));card.insertBefore(line,card.querySelector('.card-footer'));}
 const codeSelect=$('#node-form').elements.code;codeSelect.replaceChildren();const automatic=el('option','','自动识别');automatic.value='OTHER';codeSelect.append(automatic);for(const country of [...allCountries].sort((a,b)=>a.name.localeCompare(b.name,'zh-CN'))){const option=el('option','',country.name+' · '+country.code);option.value=country.code;codeSelect.append(option);}
 const renewalUI=createRenewalUI({el,api,isAdmin:()=>auth.authenticated,onChanged:()=>refresh(),toast});
 function decorateDetail(root,node){
  if(!auth.authenticated)return;const record=records.find(r=>r.public.id===node.id);if(!record)return;
  const section=el('section','admin-node-details');const heading=el('div','section-heading');heading.append(el('h3','','管理信息'),el('span','','仅管理员可见'));section.append(heading);const fields=el('div','detail-hardware');
  for(const [label,value]of [['供应商',record.providerName||'未设置'],['供应商站点',record.providerURL||'未设置'],['到期时间',formatExpiry(record.expiresAt)],['续费周期',cycleLabel(record)],['每期金额',money(record.renewalAmount,record.renewalCurrency)],['续费提醒',record.notifyRenewal?'已开启 · 提前 3 天':'已关闭'],['Agent 版本',record.agentVersion||'待上报']]){
   const item=el('dl'),content=el('dd');item.append(el('dt','',label),content);
   if(label==='供应商站点'&&record.providerURL){let url;try{url=new URL(record.providerURL);}catch{}if(url&&['https:','http:'].includes(url.protocol)&&!url.username&&!url.password){const a=el('a','provider-link',value);a.href=url.href;a.target='_blank';a.rel='noopener noreferrer';content.append(a);}else content.textContent='未设置';}else content.textContent=value;
   fields.append(item);
  }
  section.append(fields);if(record.notes){const note=el('p','private-note',record.notes);note.setAttribute('aria-label','私有备注');root.append(note);}const inspect=button('进程与服务','secondary-button detail-inspector-link',()=>inspectorUI.open(record));const inspectRow=el('dl','detail-inspector-actions'),inspectValue=el('dd');inspectRow.append(el('dt','','实时查询'),inspectValue);inspectValue.append(inspect);const hardware=root.querySelector('.detail-hardware');hardware?.classList.add('detail-hardware-admin');hardware?.append(inspectRow);root.append(section);
 }
 authUI();api('/api/session').then(v=>{auth=v;csrf=v.csrf;authUI();if(new URLSearchParams(location.search).get('login')==='1'){history.replaceState(null,'',location.pathname);openLogin();}return refresh();}).catch(e=>hooks.onLoadError(e.message));
 return {decorateCard,decorateDetail,isAdmin:()=>auth.authenticated,refresh,api,toast,getRecords:()=>records,openTerminal};
}
