import {createPager} from './pagination.mjs';
const emptyFilters=()=>({status:'all',query:'',group:null,provider:null,expiry:'all'});
export function reportedAddressRows(record){
 const ips=record.detectedIPs;
 if(ips)return [['出口 IPv4',ips.ipv4],['出口 IPv6',ips.ipv6]].filter(([,ip])=>ip);
 return record.detectedIP?[['上报 IP',record.detectedIP]]:[];
}
export function currentAddress(record){
 if(record.detectedIPs){
  const rows=reportedAddressRows(record);
  if(rows.length)return rows.map(([label,ip])=>label.slice(3)+' '+ip).join('\n');
 }
 return record.resolvedIPs?.length?record.resolvedIPs.join('、'):record.agentPeerIP||record.detectedIP||record.ip;
}
export function matchesServer(record,filters,at=Date.now()){
 const n=record.public,q=filters.query.trim().toLocaleLowerCase();
 if(q&&![n.name,record.ip,...(record.resolvedIPs||[]),record.detectedIPs?.ipv4,record.detectedIPs?.ipv6,record.detectedIP,record.agentPeerIP,n.group,record.providerName].some(s=>String(s||'').toLocaleLowerCase().includes(q)))return false;
 if(filters.group!==null&&(n.group||'')!==filters.group)return false;
 if(filters.provider!==null&&(record.providerName||'')!==filters.provider)return false;
 const expires=Date.parse(record.expiresAt),set=Number.isFinite(expires);
 if(filters.expiry==='unset'&&set)return false;
 if(filters.expiry==='expired'&&(!set||expires>at))return false;
 if(filters.expiry==='soon'&&(!set||expires<=at||expires>at+3*86400000))return false;
 if(filters.expiry==='valid'&&(!set||expires<=at+3*86400000))return false;
 return filters.status==='all'||(filters.status==='online')===Boolean(n.online);
}
export function duration(seconds){
 seconds=Math.max(0,Math.floor(Number(seconds)||0));
 if(seconds<60)return seconds+' 秒';
 if(seconds<3600)return Math.floor(seconds/60)+' 分钟';
 if(seconds<86400)return Math.floor(seconds/3600)+' 小时 '+Math.floor(seconds%3600/60)+' 分钟';
 return Math.floor(seconds/86400)+' 天 '+Math.floor(seconds%86400/3600)+' 小时';
}
const date=seconds=>seconds?new Date(seconds*1000).toLocaleString('zh-CN',{hour12:false}):'尚未收到上报';
export function createServerManagement({el,button,heading,getRecords,nodeTable,openEditor}){
 const pager=createPager(el,()=>update(),50,'服务器管理');
 let filters=emptyFilters(),box=null,body=null,tabs=null,summary=null,controls={},dialog=null,detailId='';
 function select(label,key,options){
  const wrap=el('label','form-field',label),s=el('select');s.name='server-'+key;s.dataset.serverFilter=key;
  for(const [value,text]of options)s.append(new Option(text,value));
  s.value=key==='expiry'?filters[key]:JSON.stringify(filters[key]);s.onchange=()=>{filters[key]=key==='expiry'?s.value:JSON.parse(s.value);pager.reset();update();};controls[key]=s;wrap.append(s);return wrap;
 }
 function options(key,values,all,unset){
  const s=controls[key];if(!s)return;
  const entries=[[null,all],['',unset],...[...new Set(values.filter(Boolean))].sort((a,b)=>a.localeCompare(b,'zh-CN')).map(v=>[v,v])];
  // Keep a chosen value visible if the last matching server is removed.
  if(filters[key]!==null&&!entries.some(([v])=>v===filters[key]))entries.push([filters[key],filters[key]+'（无服务器）']);
  if(JSON.stringify([...s.options].map(o=>[JSON.parse(o.value),o.text]))!==JSON.stringify(entries)){
   s.replaceChildren(...entries.map(([v,t])=>new Option(t,JSON.stringify(v))));s.value=JSON.stringify(filters[key]);
  }
 }
 function render(panel){
  box=el('section','glass admin-box');const title=heading('服务器管理','搜索、筛选与连接状态');
  title.append(button('添加服务器','primary-button',()=>openEditor(),'plus'));box.append(title);
  const fields=el('div','server-search-fields'),searchWrap=el('label','form-field server-search','搜索服务器'),search=el('input');
  search.type='search';search.name='server-query';search.placeholder='名称、IP、分组或供应商';search.maxLength=160;search.value=filters.query;
  let composing=false;const changed=()=>{filters.query=search.value;pager.reset();update();};
  search.addEventListener('compositionstart',()=>composing=true);search.addEventListener('compositionend',()=>{composing=false;changed();});
  search.addEventListener('input',()=>{if(!composing)changed();});controls.query=search;searchWrap.append(search);
  fields.append(searchWrap,select('分组','group',[]),select('供应商','provider',[]),select('到期状态','expiry',[['all','全部到期状态'],['soon','3 天内到期'],['expired','已到期'],['valid','3 天后到期'],['unset','未设置']]));
  const reset=button('重置','secondary-button server-filter-reset',()=>{filters=emptyFilters();pager.reset();search.value='';for(const key of ['group','provider','expiry'])controls[key].value=key==='expiry'?filters[key]:JSON.stringify(filters[key]);update();});
  fields.append(reset);tabs=el('div','status-tabs admin-server-filters');tabs.setAttribute('role','group');tabs.setAttribute('aria-label','按服务器状态筛选');
  for(const [id,label] of [['all','全部'],['online','在线'],['offline','离线']]){
   const tab=button(label,'tab',()=>{filters.status=id;pager.reset();update();});tab.dataset.serverStatus=id;tab.append(el('span'));tabs.append(tab);
  }
  summary=el('p','server-filter-summary');summary.setAttribute('role','status');body=el('div','server-results');
  box.append(fields,tabs,summary,body,pager.root);panel.append(box);update();
 }
 function update(){
  if(dialog?.open)drawDetail();
  if(!box?.isConnected)return;
  const records=getRecords(),at=Date.now(),matched=records.filter(r=>matchesServer(r,{...filters,status:'all'},at)),rows=matched.filter(r=>matchesServer(r,filters,at)).sort((a,b)=>(a.public.order||0)-(b.public.order||0));
  options('group',records.map(r=>r.public.group),'全部分组','未分组');options('provider',records.map(r=>r.providerName),'全部供应商','未设置');
  for(const b of tabs.children){
   const id=b.dataset.serverStatus,count=matched.filter(r=>id==='all'||(id==='online')===Boolean(r.public.online)).length;
   b.classList.toggle('active',id===filters.status);b.setAttribute('aria-pressed',String(id===filters.status));b.querySelector('span').textContent=count;
  }
  const range=pager.range(rows.length);
  const ordering=Object.entries(filters).every(([k,v])=>v===emptyFilters()[k]);
  summary.textContent='显示 '+rows.length+' / '+records.length+' 台'+(ordering?' · 拖动行或使用箭头调整看板顺序':'');
  const focused=document.activeElement,focusId=focused?.closest('tr')?.dataset.id,focusLabel=focused?.getAttribute('aria-label')||focused?.textContent;
  body.replaceChildren(nodeTable(false,rows.slice(range.start,range.end),{ordering,health:true,emptyMessage:!records.length?'还没有服务器，添加第一台开始监控。':'没有符合筛选条件的服务器。'}));
  if(focusId)for(const row of body.querySelectorAll('tr[data-id]'))if(row.dataset.id===focusId){
   [...row.querySelectorAll('button')].find(b=>(b.getAttribute('aria-label')||b.textContent)===focusLabel)?.focus({preventScroll:true});break;
  }
 }
 function health(r){
  const wrap=el('div','server-health'),h=r.health||{},at=h.checkedAt||Date.now()/1000;
  const last=el('small','',h.lastReportAt?'上报于 '+duration(at-h.lastReportAt)+'前':'尚未收到上报');last.title=date(h.lastReportAt);wrap.append(last);
  if(!r.public.online&&h.offlineSince)wrap.append(el('small','server-offline-duration','离线'+(h.offlineEstimated?'约 ':' ')+duration(at-h.offlineSince)));
  if(!r.public.online&&h.lastError&&h.lastErrorAt>=(h.offlineSince||0)-15)wrap.append(el('small','server-last-error',h.lastError));
  wrap.append(button('排查信息','table-button',()=>openDetail(r.public.id)));return wrap;
 }
 function openDetail(id){
  detailId=id;if(!dialog){
   dialog=el('dialog','node-dialog server-diagnostics');dialog.setAttribute('aria-label','连接排查信息');document.body.append(dialog);
   dialog.addEventListener('close',()=>{detailId='';dialog.replaceChildren();});
  }
  drawDetail();dialog.showModal();
 }
 function drawDetail(){
  const r=getRecords().find(r=>r.public.id===detailId);if(!r){dialog?.close();return;}
  const h=r.health||{},inner=el('div','dialog-inner'),title=el('div','admin-section-heading');
  title.append(el('h2','',r.public.name+' · 连接信息'),button('关闭','secondary-button',()=>dialog.close()));
  inner.append(title);const fields=el('dl','server-diagnostic-fields');
  for(const [label,value]of [
   ['当前状态',r.public.pending?'待接入':r.public.online?'在线':'离线'],['SSH 地址',(r.ip.includes(':')?'['+r.ip+']':r.ip)+':'+r.port],
   ...(r.resolvedIPs?[['解析 IP',r.resolvedIPs.join('、')||'等待域名解析']]:[]),['Agent 接入 IP',r.agentPeerIP||'待连接'],...(reportedAddressRows(r).length?reportedAddressRows(r):[['出口 IP','待上报']]),
   ['最后有效上报',date(h.lastReportAt)],['离线开始',h.offlineSince?date(h.offlineSince)+(h.offlineEstimated?'（依据上报超时估算）':''):'—'],
   ['最近通讯错误',h.lastError||'暂无记录'],['错误记录时间',h.lastErrorAt?date(h.lastErrorAt):'—'],
   ['Agent 版本',r.agentVersion||'待上报'],['最近部署',r.deployMessage||'暂无记录'],
   ['管理任务',r.management?.message||'暂无任务']
  ])fields.append(el('dt','',label),el('dd','',value));
  inner.append(fields,el('p','form-footnote','连接中断记录不能单独确定故障原因。请核对服务器电源、网络、时间、证书及 Agent 服务状态。'),
   el('p','form-footnote','在服务器控制台检查 Agent：'),el('pre','server-check-command','systemctl status vistart-probe-agent\njournalctl -u vistart-probe-agent -n 50 --no-pager'));
  const focused=dialog.contains(document.activeElement);dialog.replaceChildren(inner);if(focused)dialog.querySelector('button')?.focus({preventScroll:true});
 }
 function clear(){filters=emptyFilters();pager.reset();dialog?.close();box=null;}
 return {render,update,health,clear};
}
