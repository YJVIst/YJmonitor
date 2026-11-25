import {historyPath,historyTransition} from './board-history.mjs';
import {createPager} from './pagination.mjs';
import {refreshAppearance} from './appearance.mjs';
import {initAdmin} from './admin.mjs';
import {countryByCode,normalizedCountry} from './countries.mjs';
import {createNetworkUI} from './network.mjs';

const $=selector=>document.querySelector(selector);
const NS='http://www.w3.org/2000/svg';
const format=new Intl.NumberFormat('zh-CN',{maximumFractionDigits:2});
const nodes=[];
let admin=null;
const state={status:'all',country:'all',group:'all',query:'',sort:'default',step:0,dialogId:null};
const glyphs={
 settings:['M9 3h6l1 3 3 1 2 5-2 5-3 1-1 3H9l-1-3-3-1-2-5 2-5 3-1 1-3z','M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0'],
 terminal:['M3 4h18v16H3z','m7 9 3 3-3 3m6 0h4'],shield:['M12 3 4 6v6c0 4 5 8 8 9 3-1 8-5 8-9V6l-8-3z','m8 12 3 3 5-6'],
 plus:['M12 5v14M5 12h14'],edit:['m15 4 5 5-10 10-6 1 1-6L15 4z','m12 7 5 5'],trash:['M4 6h16M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7m4-7v7'],
 grid:['M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z'],check:['m5 12 4 4L19 6'],logout:['M10 4H4v16h6M9 12h12m-4-4 4 4-4 4'],activity:['M3 12h4l3-8 4 16 3-8h4'],

 search:['M21 21l-4.4-4.4','M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0'],
 globe:['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0','M3 12h18','M12 3c4 4 4 14 0 18-4-4-4-14 0-18'],
 sort:['M8 4v16m-4-4 4 4 4-4','M15 5h6m-6 5h4m-4 5h2'],
 chevron:['m8 10 4 4 4-4'],arrow:['m9 6 6 6-6 6'],
 cpu:['M7 7h10v10H7z','M9 10h5v4H9z','M9 3v4m6-4v4M9 17v4m6-4v4M3 9h4m-4 6h4m10-6h4m-4 6h4'],
 memory:['M4 7h16v10H4z','M7 10h3v3H7zM14 10h3v3h-3z','M7 17v3m5-3v3m5-3v3'],
 disk:['M5 5h14l3 10H2L5 5z','M2 15v5h20v-5','M17 17.5h1m-5 0h1'],
 swap:['M4 8h15m-4-4 4 4-4 4','M20 16H5m4-4-4 4 4 4'],
 clock:['M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0','M12 7v5l3 2'],
 refresh:['M20 8a8 8 0 0 0-14-3L3 8','M3 3v5h5','M4 16a8 8 0 0 0 14 3l3-3','M21 21v-5h-5'],
 moon:['M20.8 13A9 9 0 0 1 11 3.2 9 9 0 1 0 20.8 13z'],
 sun:['M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0','M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.4 1.4m11.2 11.2L19 19M5 19l1.4-1.4M17.6 6.4 19 5'],
 close:['m6 6 12 12M6 18 18 6'],server:['M4 3h16v7H4zM4 14h16v7H4z','M7 6.5h.1M7 17.5h.1M12 6.5h5M12 17.5h5']
};
function el(tag,cls,text){const node=document.createElement(tag);if(cls)node.className=cls;if(text!==undefined)node.textContent=String(text);return node;}
function svgEl(tag,attrs={}){const node=document.createElementNS(NS,tag);for(const [k,v]of Object.entries(attrs))node.setAttribute(k,String(v));return node;}
function icon(name){const s=svgEl('svg',{viewBox:'0 0 24 24',fill:'none',stroke:'currentColor','stroke-width':1.5,'stroke-linecap':'round','stroke-linejoin':'round','aria-hidden':'true'});for(const d of glyphs[name]??[])s.append(svgEl('path',{d}));return s;}
function flag(code){const country=countryByCode.get(normalizedCountry(code));if(!country)return icon('globe');const image=el('img','country-flag');image.src=country.flag;image.alt=country.name+'国旗';image.width=30;image.height=20;image.decoding='async';return image;}
const networkUI=createNetworkUI({el});
const finite=value=>typeof value==='number'&&Number.isFinite(value);
const hasSwap=node=>finite(node.swap?.total)&&node.swap.total>0;
const percent=(used,total)=>finite(used)&&finite(total)&&total>0?Math.min(100,Math.max(0,used/total*100)):null;
const display=value=>finite(value)?format.format(value):'—';
const locationLabel=node=>node.country.endsWith(node.city)?node.country:node.country+' · '+node.city;
function diskTotal(node){return {total:node.disks.reduce((sum,d)=>sum+d.total,0),used:node.disks.every(d=>finite(d.used))?node.disks.reduce((sum,d)=>sum+d.used,0):null};}
function statusBadge(node){const badge=el('span','status-badge'+(node.online?'':' is-offline'));badge.append(el('i','status-dot '+(node.online?'online':'offline')),document.createTextNode(node.pending?'待接入':node.online?'在线':'离线'));return badge;}
function meter(value,label){if(!finite(value)){const missing=el('div','metric-meter offline-meter');missing.setAttribute('role','img');missing.setAttribute('aria-label',label+'暂无数据');return missing;}const m=el('meter','metric-meter'+(value>=85?' warning':''));m.min=0;m.max=100;m.value=value;m.setAttribute('aria-label',label);m.setAttribute('aria-valuetext',value.toFixed(1)+'%');return m;}
function resource(label,kind,values,online=true){const box=el('div','resource-block');const heading=el('span','metric-label');heading.append(icon(kind),document.createTextNode(label));const value=el('div','resource-value');value.append(document.createTextNode(online?display(values?.used):'—'),el('span','resource-total',values?.total>0?'/ '+display(values.total)+' GiB':''));const ratio=online&&values?percent(values.used,values.total):null;box.append(heading,value,meter(ratio,label+'使用率'));const caption=el('div','resource-caption');caption.append(el('span','',finite(ratio)?ratio.toFixed(1)+'%':online?'未启用':'暂无数据'),el('span','','已用 / 总量'));box.append(caption);return box;}
function openButton(node,cls,text){const b=el('button',cls,text);b.type='button';b.dataset.node=node.id;b.setAttribute('aria-label',`查看 ${node.name} 详情`);b.addEventListener('click',()=>openDetail(node.id));return b;}
function makeCard(node){const article=el('article','server-card'+(node.online?'':' is-offline'));article.dataset.id=node.id;article.setAttribute('aria-labelledby','title-'+node.id);const heading=el('div','card-heading');const flagWrap=el('span','flag-wrap');flagWrap.append(flag(node.code));const identity=el('div','node-identity');const title=el('h2','node-title');title.id='title-'+node.id;title.append(openButton(node,'',node.name));identity.append(title,el('p','node-location',locationLabel(node)));if(node.group||node.pinned)identity.append(el('p','node-group',(node.pinned?'置顶 · ':'')+(node.group||'未分组')));const badges=el('div','node-badges');badges.append(statusBadge(node),el('span','latency-badge'));heading.append(flagWrap,identity,badges);const hardware=el('div','hardware-row');const model=el('span','cpu-model',node.cpuModel);model.title=node.cpuModel;hardware.append(icon('cpu'),model,el('span','core-count',(node.cores||'—')+' vCPU'));const content=el('div','card-metrics');const footer=el('div','card-footer');const up=el('span','uptime');up.append(icon('clock'),document.createTextNode(node.online?'运行 '+node.uptime:(node.pending?'等待首次上报':'最后上报 '+node.lastSeenMinutes+' 分钟前')));const detail=openButton(node,'detail-link','详情');detail.append(icon('arrow'));footer.append(up,detail);article.append(heading,hardware,content,footer);patchLatencyBadge(article.querySelector('.latency-badge'),node);admin?.decorateCard(article,node);return article;}
function latencyReading(node){return node?.online&&finite(node.latencyMs)&&node.latencyMs>=0?Math.round(node.latencyMs)+' ms':'—';}
function patchLatencyBadge(badge,node){
 if(!badge)return;
 const value=latencyReading(node);
 if(badge.dataset.value===value)return;
 badge.dataset.value=value;
 badge.textContent='通讯 '+value;
 badge.title='Agent 与主控通过 WSS 实测的往返延时';
 badge.setAttribute('aria-label',value==='—'?'与主控的通讯延时暂无数据':'与主控的通讯延时：'+value);
 badge.classList.toggle('is-muted',value==='—');
}
const cardViews=new Map();
const visibleCards=new Set();
const reducedMotion=matchMedia('(prefers-reduced-motion: reduce)');
const nearViewport=new IntersectionObserver(entries=>{
 for(const {target,isIntersecting} of entries){
  const view=cardViews.get(target.dataset.id);
  if(view?.card!==target)continue;
  if(isIntersecting){visibleCards.add(view);view.sampleAt=0;updateMetrics(view);}
  else{visibleCards.delete(view);view.motion?.cancel();view.motion=null;}
 }
});
function setText(element,value){if(element.textContent!==value)element.textContent=value;}
function buildMetrics(view){
 const {card,node}=view, content=card.querySelector('.card-metrics');
 const cpu=el('div','cpu-block'),amount=el('div'),value=el('div','cpu-value'),number=el('span');
 value.append(number);if(node.online)value.append(el('span','unit','%'));
 amount.append(el('span','metric-label','CPU 使用率'),value);
 const chart=el('div',node.online?'sparkline live-chart':'sparkline-offline',node.online?'':'等待重新连接');
 const strip=el('div','chart-strip');if(node.online)chart.append(strip);cpu.append(amount,chart);
 const grid=el('div','resource-grid'),resources=[];
 for(const [label,kind] of [['内存','memory'],['磁盘','disk']]){
  const row=resource(label,kind,{used:0,total:1},node.online);
  const track=el('div','board-meter'),bar=el('i');track.append(bar);
  track.setAttribute('role','meter');track.setAttribute('aria-label',label+'使用率');
  row.querySelector('.metric-meter').replaceWith(track);
  resources.push({row,bar,track,value:row.querySelector('.resource-value').firstChild,total:row.querySelector('.resource-total'),ratio:row.querySelector('.resource-caption span')});grid.append(row);
 }
 let swap;
 if(hasSwap(node)){const label=el('span','swap-label'),wrap=el('div','memory-swap');swap=el('span','swap-usage');label.append(icon('swap'),document.createTextNode('Swap'));wrap.append(label,swap);resources[0].row.classList.add('memory-resource');resources[0].row.append(wrap);}
 const net=networkUI.card(node);content.replaceChildren(cpu,grid,net);
 Object.assign(view,{number,value,strip,chart,resources,swap,net});
}
function plotHistory(view,samples,motion,at,defaultPeriod){
 const previous=view.history||[];
 if(samples.length===previous.length&&samples.every((value,index)=>value===previous[index]))return;
 const elapsed=at-(view.chartAt??at),animation=view.motion;
 const remaining=animation&&view.duration?view.offset*Math.max(0,1-(animation.currentTime||0)/view.duration):0;
 const next=historyTransition(previous,samples,motion&&elapsed<5500,remaining);
 animation?.cancel();view.motion=null;
 view.history=next.history;view.chartAt=at;
 if(!view.paths){
  const svg=svgEl('svg',{viewBox:'0 0 112 42',preserveAspectRatio:'none','aria-hidden':'true'});
  view.paths={fill:svgEl('path',{class:'spark-fill'}),line:svgEl('path',{class:'spark-line'})};
  svg.append(view.paths.fill,view.paths.line);view.strip.append(svg);
 }
 const path=historyPath(next.points);
 view.paths.line.setAttribute('d',path.line);view.paths.fill.setAttribute('d',path.fill);
 view.chart.classList.toggle('warning',samples.at(-1)>=85);
 if(next.offset){
  view.duration=elapsed>=600?Math.min(4500,elapsed):defaultPeriod;
  view.offset=next.offset;
  view.motion=view.strip.animate([{transform:'translateX('+next.offset+'%)'},{transform:'translateX(0)'}],{duration:view.duration,easing:'linear'});
 }
}
function updateMetrics(view){
 const n=view.node;
 if(!view.resources)buildMetrics(view);
 const at=performance.now(),motion=!!view.sampleAt&&at-view.sampleAt<2500&&n.online&&!reducedMotion.matches;
 const period=n.demo?1000:3000;
 view.sampleAt=at;
 const painted=view.lastPainted;
 if(!painted||n.cpu!==painted.cpu){
  setText(view.number,n.online&&finite(n.cpu)?n.cpu.toFixed(1):'—');
  view.value.classList.toggle('warning',n.cpu>=85);
 }
 if(n.online)plotHistory(view,n.history,motion,at,period);
 if(!painted||painted.memory?.used!==n.memory?.used||painted.memory?.total!==n.memory?.total)
  patchResource(view.resources[0],n.memory,n.online,motion?period:0);
 const oldDisks=painted?.disks;
 if(!oldDisks||oldDisks.length!==n.disks.length||n.disks.some((disk,i)=>disk.used!==oldDisks[i].used||disk.total!==oldDisks[i].total))
  patchResource(view.resources[1],diskTotal(n),n.online,motion?period:0);
 if(view.swap&&(!painted||painted.swap?.used!==n.swap.used||painted.swap?.total!==n.swap.total)){
  setText(view.swap,n.online?display(n.swap.used)+' / '+display(n.swap.total)+' GiB':'暂无数据');
  view.swap.classList.toggle('swap-warning',n.online&&percent(n.swap.used,n.swap.total)>=85);
 }
 networkUI.updateCard(view.net,n);
 view.lastPainted=n;
}
function patchResource(r,values,online,duration){
 const used=online?values?.used:null,total=values?.total,ratio=percent(used,total),value=display(used);
 if(r.value.data!==value)r.value.data=value;
 setText(r.total,total>0?'/ '+display(total)+' GiB':'');
 setText(r.ratio,ratio!==null?ratio.toFixed(1)+'%':online?'未启用':'暂无数据');
 r.bar.style.transitionDuration=duration+'ms';r.bar.style.transform='scaleX('+((ratio||0)/100)+')';
 r.track.classList.toggle('warning',ratio>=85);
 if(ratio===null)r.track.removeAttribute('aria-valuenow');else r.track.setAttribute('aria-valuenow',ratio.toFixed(1));
}
function sameCard(a,b){
 return a.id===b.id&&a.name===b.name&&a.group===b.group&&a.pinned===b.pinned&&a.country===b.country&&a.city===b.city&&
  a.code===b.code&&a.online===b.online&&a.pending===b.pending&&a.cpuModel===b.cpuModel&&a.cores===b.cores&&hasSwap(a)===hasSwap(b);
}
function stableListEntry(old,n){
 if(!old||!sameCard(old,n)||old.order!==n.order||old.arch!==n.arch)return false;
 if(!n.online)return true;
 if(state.sort==='cpu')return old.cpu===n.cpu;
 if(state.sort==='memory')return percent(old.memory.used,old.memory.total)===percent(n.memory.used,n.memory.total);
 return true;
}
function patchCard(view,n,record){
 if(record&&view.ip){
  setText(view.ip,record.currentIP);
  if(view.ip.parentElement.title!==record.ip)view.ip.parentElement.title=record.ip;
 }
 if(view.node===n)return;
 const old=view.node;view.node=n;
 if(old.latencyMs!==n.latencyMs)patchLatencyBadge(view.latency,n);
 if(old.uptime!==n.uptime||old.lastSeenMinutes!==n.lastSeenMinutes){
  const text=n.online?'运行 '+n.uptime:n.pending?'等待首次上报':'最后上报 '+n.lastSeenMinutes+' 分钟前';
  if(view.uptime.data!==text)view.uptime.data=text;
 }
 if(visibleCards.has(view))updateMetrics(view);
}
function adminRecords(){return admin?.isAdmin()?new Map(admin.getRecords().map(r=>[r.public.id,r])):null;}
let boardAdmin=false;
function syncCards(list){
 const grid=$('#server-grid'),wanted=new Set(list.map(n=>n.id));
 for(const [id,v] of cardViews)if(!wanted.has(id)){nearViewport.unobserve(v.card);visibleCards.delete(v);v.motion?.cancel();v.card.remove();cardViews.delete(id);}
 grid.classList.toggle('many-nodes',list.length>8);
 const isAdmin=admin?.isAdmin();
 const records=adminRecords();boardAdmin=!!isAdmin;
 let cursor=grid.firstElementChild;
 for(const n of list){
  const record=records?.get(n.id);
  let v=cardViews.get(n.id);
  if(!v||v.admin!==isAdmin||!sameCard(v.node,n)){
   const card=makeCard(n);
   if(v){if(cursor===v.card)cursor=card;nearViewport.unobserve(v.card);visibleCards.delete(v);v.motion?.cancel();v.card.replaceWith(card);}
   v={node:n,card,admin:isAdmin,latency:card.querySelector('.latency-badge'),uptime:card.querySelector('.uptime').lastChild,ip:card.querySelector('.admin-ip-row code')};cardViews.set(n.id,v);nearViewport.observe(card);
  }
  if(v.card!==cursor)grid.insertBefore(v.card,cursor);
  cursor=v.card.nextElementSibling;
  patchCard(v,n,record);
 }
}
function filtered(){const q=state.query.normalize('NFKC').toLocaleLowerCase();const list=nodes.filter(n=>(state.status==='all'||(state.status==='online')===n.online)&&(state.country==='all'||n.country===state.country)&&(state.group==='all'||'g:'+n.group===state.group)&&(!q||[n.name,n.group,n.country,n.city,n.cpuModel,n.arch].join(' ').normalize('NFKC').toLocaleLowerCase().includes(q)));if(state.sort==='cpu')list.sort((a,b)=>(b.online?b.cpu:-1)-(a.online?a.cpu:-1));if(state.sort==='memory')list.sort((a,b)=>(b.online?percent(b.memory.used,b.memory.total):-1)-(a.online?percent(a.memory.used,a.memory.total):-1));if(state.sort==='name')list.sort((a,b)=>a.name.localeCompare(b.name));if(state.sort==='default')list.sort((a,b)=>Number(!!b.pinned)-Number(!!a.pinned)||(a.order||0)-(b.order||0));return list;}
const boardPager=createPager(el,()=>{render();$('#server-grid').scrollIntoView({block:'start'});},60,'服务器看板');
$('#server-grid').after(boardPager.root);
function render(){const focused=document.activeElement;const focusedId=focused?.dataset?.node;const focusedClass=focused?.className;const list=filtered(),range=boardPager.range(list.length);syncCards(list.slice(range.start,range.end));$('#empty-state').hidden=list.length!==0;$('#server-grid').hidden=list.length===0;$('#result-count').textContent=list.length===nodes.length?'全部 '+list.length+' 台服务器':'显示 '+list.length+' / '+nodes.length+' 台服务器';$('#empty-description').textContent=state.query?'未找到与“'+state.query+'”匹配的节点。':'这个地区暂时没有符合条件的服务器。';for(const b of document.querySelectorAll('[data-status]')){const active=b.dataset.status===state.status;b.classList.toggle('active',active);b.setAttribute('aria-pressed',String(active));}if(focusedId&&!focused.isConnected){const restore=Array.from(document.querySelectorAll('[data-node]')).find(b=>b.dataset.node===focusedId&&b.className===focusedClass);restore?.focus({preventScroll:true});}}
function applyTheme(theme){document.documentElement.dataset.theme=theme;$('#theme-toggle').setAttribute('aria-label',theme==='dark'?'切换到浅色主题':'切换到深色主题');$('#theme-toggle').replaceChildren(icon(theme==='dark'?'sun':'moon'));try{localStorage.setItem('vistart-probe-theme',theme);}catch{}}
let storedTheme;try{storedTheme=localStorage.getItem('vistart-probe-theme');}catch{}
applyTheme(['light','dark'].includes(storedTheme)?storedTheme:matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light');
$('#theme-toggle').addEventListener('click',()=>applyTheme(document.documentElement.dataset.theme==='dark'?'light':'dark'));
for(const [selector,name]of [['.search-icon','search'],['.region-icon','globe'],['.sort-icon','sort'],['.empty-icon','search'],['#dialog-close','close']])$(selector).replaceChildren(icon(name));
for(const elem of document.querySelectorAll('.select-chevron'))elem.replaceChildren(icon('chevron'));
for(const country of [...new Set(nodes.map(n=>n.country))]){const opt=el('option','',country);opt.value=country;$('#country-filter').append(opt);}
for(const button of document.querySelectorAll('[data-status]'))button.addEventListener('click',()=>{state.status=button.dataset.status;boardPager.reset();render();});
$('#search').addEventListener('input',event=>{state.query=event.target.value.trim().slice(0,100);boardPager.reset();render();});
$('#country-filter').addEventListener('change',event=>{state.country=event.target.value;boardPager.reset();render();});
$('#group-filter').addEventListener('change',event=>{state.group=event.target.value;boardPager.reset();render();});
$('#sort').addEventListener('change',event=>{state.sort=event.target.value;boardPager.reset();render();});
$('#clear-filters').addEventListener('click',()=>{state.query='';state.status='all';state.country='all';state.group='all';$('#group-filter').value='all';state.sort='default';$('#search').value='';$('#country-filter').value='all';$('#sort').value='default';boardPager.reset();render();$('#search').focus();});
function patchDetailMeter(parent,value,label){
 let track=parent.querySelector('.metric-meter');
 if(!track||((track.tagName==='METER')!==finite(value))){
  const next=meter(value,label);if(track)track.replaceWith(next);else parent.append(next);return;
 }
 if(finite(value)&&track.value!==value){
  track.value=value;track.classList.toggle('warning',value>=85);track.setAttribute('aria-valuetext',value.toFixed(1)+'%');
 }
}
function detailUsageRow(className,showPercent=false){
 const item=el('li','detail-usage-item '+className),row=el('div','detail-usage-head'),name=el('strong');
 const values=el('span','detail-usage-values'),size=el('span'),usage=showPercent?el('span','detail-usage-percent'):null;
 values.append(size);if(usage)values.append(usage);row.append(name,values);item.append(row);
 function update(label,amount,online){
  if(name.textContent!==label){setText(name,label);item.querySelector('.metric-meter')?.remove();}
  const ratio=online?percent(amount?.used,amount?.total):null;
  setText(size,(online?display(amount?.used):'—')+' / '+display(amount?.total)+' GiB');
  if(usage)setText(usage,finite(ratio)?ratio.toFixed(1)+'%':'');
  patchDetailMeter(item,ratio,label+'使用率');
 }
 return {item,update};
}
let detailView=null;
function createDetail(node){
 const root=$('#dialog-content'),heading=el('div','dialog-header'),flagWrap=el('span','flag-wrap');
 const titles=el('div'),title=el('h2'),location=el('p','node-location'),badge=statusBadge(node);
 title.id='dialog-title';titles.append(title,location);heading.append(flagWrap,titles,badge);
 const hardware=el('div','detail-hardware'),fields=[];
 for(const label of ['CPU 型号','核心 / 架构','操作系统','主控通讯延时','运行时间']){
  const block=el('dl'),field=el('dd');block.append(el('dt','',label),field);
  if(label==='主控通讯延时')block.dataset.metric='latency';
  hardware.append(block);fields.push(field);
 }
 const chart=el('section','detail-chart'),chartHeader=el('div','section-heading'),reading=el('span');
 chartHeader.append(el('h3','','CPU 使用率'),reading);
 const viewport=el('div','live-chart'),strip=el('div','chart-strip'),graph={chart:viewport,strip};
 viewport.setAttribute('role','img');viewport.append(strip);
 const axis=el('div','chart-axis');axis.append(el('span','','最近 2 分钟'),el('span','','现在'));chart.append(chartHeader,viewport,axis);
 const notice=el('p','dialog-notice'),resources=el('section','detail-memory'),memoryHeader=el('div','section-heading');
 memoryHeader.append(el('h3','','内存'),el('span','','已用 / 总量'));
 const memoryList=el('ul','detail-usage-list detail-resource-row');
 const memory=detailUsageRow('memory-item',true),swap=detailUsageRow('swap-item',true);
 memoryList.append(memory.item,swap.item);resources.append(memoryHeader,memoryList);
 const disks=el('section'),diskHeader=el('div','section-heading'),count=el('span'),list=el('ul','detail-usage-list disk-list'),rows=[];
 diskHeader.append(el('h3','','磁盘'),count);disks.append(diskHeader,list);
 const network=networkUI.detail();
 root.replaceChildren(heading,hardware,chart,notice,resources,disks,network.section);
 const updateAdmin=admin?.decorateDetail(root,node);
 let previous;
 function update(n){
  if(!previous||previous.code!==n.code)flagWrap.replaceChildren(flag(n.code));
  setText(title,n.name);setText(location,locationLabel(n));
  if(!previous||previous.online!==n.online||previous.pending!==n.pending){
   badge.classList.toggle('is-offline',!n.online);badge.firstChild.className='status-dot '+(n.online?'online':'offline');
   badge.lastChild.data=n.pending?'待接入':n.online?'在线':'离线';
   chart.hidden=!n.online;notice.hidden=n.online;
  }
  const hardwareValues=[n.cpuModel,n.cores+' vCPU · '+n.arch,n.system,latencyReading(n),n.online?n.uptime:'离线 · 等待上报'];
  fields.forEach((field,i)=>setText(field,hardwareValues[i]));
  if(n.online){
   const cpu=finite(n.cpu)?n.cpu.toFixed(1)+'%':'—';
   setText(reading,cpu+(n.demo?' · 演示趋势':' · 实时采样'));
   const label=(n.demo?'CPU 使用率演示曲线，当前 ':'CPU 使用率曲线，当前 ')+cpu;
   if(viewport.getAttribute('aria-label')!==label)viewport.setAttribute('aria-label',label);
   plotHistory(graph,n.history,previous?.online&&!reducedMotion.matches,performance.now(),n.demo?1000:3000);
  }else{
   graph.motion?.cancel();graph.motion=null;graph.history=null;
   setText(notice,'最后一次上报于 '+n.lastSeenMinutes+' 分钟前。节点离线后暂停显示使用率，避免将过期数据作为当前状态。');
  }
  memory.update('物理内存',n.memory,n.online);
  const has=hasSwap(n);if(swap.item.hidden===has)swap.item.hidden=!has;if(has)swap.update('Swap',n.swap,n.online);
  setText(count,n.disks.length+' 个卷 · 容量单位 GiB');
  while(rows.length>n.disks.length)rows.pop().item.remove();
  n.disks.forEach((disk,i)=>{
   if(!rows[i]){const view=detailUsageRow('disk-item');rows.push(view);list.append(view.item);}
   rows[i].update(disk.name,disk,n.online);
  });
  network.update(n);updateAdmin?.();previous=n;
 }
 return {id:node.id,update,destroy:()=>graph.motion?.cancel()};
}
function showDetail(node){
 if(detailView?.id!==node.id){detailView?.destroy();detailView=createDetail(node);}
 detailView.update(node);
}
let dialogTrigger=null;
function openDetail(id){const node=nodes.find(n=>n.id===id);if(!node)return;dialogTrigger={id,detail:document.activeElement?.classList.contains('detail-link')};state.dialogId=id;showDetail(node);$('#node-dialog').showModal();$('#dialog-close').focus();}
$('#dialog-close').addEventListener('click',()=>$('#node-dialog').close());
$('#node-dialog').addEventListener('close',()=>{detailView?.destroy();detailView=null;state.dialogId=null;if(dialogTrigger){const card=Array.from(document.querySelectorAll('.server-card')).find(c=>c.dataset.id===dialogTrigger.id);card?.querySelector(dialogTrigger.detail?'.detail-link':'.node-title button')?.focus({preventScroll:true});dialogTrigger=null;}});
$('#node-dialog').addEventListener('keydown',event=>{if(event.key!=='Tab')return;const controls=Array.from(event.currentTarget.querySelectorAll('button:not([disabled]),a[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])')).filter(e=>e.getClientRects().length>0);const first=controls[0],last=controls.at(-1);if(!first){event.preventDefault();event.currentTarget.focus();return;}if(controls.length===1||(event.shiftKey&&document.activeElement===first)||(!event.shiftKey&&document.activeElement===last)){event.preventDefault();(event.shiftKey?last:first).focus();}});
$('#node-dialog').addEventListener('click',event=>{if(event.target!==$('#node-dialog'))return;const rect=event.target.getBoundingClientRect();if(event.clientX<rect.left||event.clientX>rect.right||event.clientY<rect.top||event.clientY>rect.bottom)event.target.close();});
document.addEventListener('keydown',event=>{if(event.key==='/'&&!event.ctrlKey&&!event.metaKey&&!event.altKey&&!$('#node-dialog').open&&!['INPUT','TEXTAREA','SELECT'].includes(document.activeElement.tagName)){event.preventDefault();$('#search').focus();}});
let appliedSiteName='';
function applySite(site){
 const name=typeof site?.name==='string'&&site.name.trim()?site.name:'服务器监控';
 if(name===appliedSiteName)return;appliedSiteName=name;
 for(const element of document.querySelectorAll('[data-site-name]'))element.textContent=name;
 const brand=$('.brand');brand.setAttribute('aria-label',name+' 首页');brand.title=name;
 document.title='服务器状态 · '+name;
 document.querySelector('meta[name="description"]').content=name+' 服务器状态看板。查看节点所在地区、CPU、内存、Swap、磁盘与网络流量。';
}
function updateFilterOptions(select,values,label,prefix){
 const options=select.options;
 if(options.length===values.length+1&&values.every((value,i)=>options[i+1].value===prefix+value))return;
 select.replaceChildren(new Option(label,'all'),...values.map(value=>new Option(value||'未分组',prefix+value)));
}
function applyData(list,site,preview,themeRevision){
 refreshAppearance(themeRevision);
 applySite(site);
 const oldById=list.some(n=>n.demo)?new Map(nodes.map(n=>[n.id,n])):null;
 let layoutUnchanged=nodes.length>0&&nodes.length===list.length&&boardAdmin===admin?.isAdmin();
 state.step++;
 nodes.splice(0,nodes.length,...list.map((n,index)=>{
  const c=countryByCode.get(normalizedCountry(n.code));if(c)n.country=c.name;
  const old=oldById?.get(n.id);
  if(n.demo&&n.online&&old?.demo&&finite(n.cpu)){
   n.cpu=Math.round(Math.max(0,Math.min(100,n.cpu+Math.sin(state.step*.7+index)*2.1))*10)/10;
   n.history=[...old.history.slice(-39),n.cpu];
  }
  if(layoutUnchanged&&!stableListEntry(nodes[index],n))layoutUnchanged=false;
  return n;
 }));
 if(!layoutUnchanged){
  const online=nodes.filter(n=>n.online).length,offline=nodes.length-online;
  $('#node-total').textContent=nodes.length;$('#online-total').textContent=online;$('#offline-total').textContent=offline;
  for(const [key,count]of [['all',nodes.length],['online',online],['offline',offline]])document.querySelector('[data-status="'+key+'"] span').textContent=count;
  const countries=[...new Set(nodes.map(n=>n.country))],groups=[...new Set(nodes.map(n=>n.group||''))].sort();
  updateFilterOptions($('#country-filter'),countries,'全部地区','');
  updateFilterOptions($('#group-filter'),groups,'全部分组','g:');
  if(!countries.includes(state.country))state.country='all';
  if(!groups.some(g=>'g:'+g===state.group))state.group='all';
  $('#country-filter').value=state.country;$('#group-filter').value=state.group;
 }
 const mixed=preview&&list.some(n=>!n.demo);
 const label=mixed?'实时与演示节点':preview?'界面预览':'实时监控';
 if($('.demo-label').textContent!==label){
  $('.demo-label').replaceChildren(el('span'),document.createTextNode(label));
  $('.footer>span').replaceChildren(el('span','footer-dot'),document.createTextNode(mixed?'真实节点由 Agent 上报，演示节点使用模拟数据':preview?'演示数据 · 尚未接入 Agent':'监测数据由 Agent 实时上报'));
 }
 if(!$('#main-content').hidden){
  if(layoutUnchanged){
   const records=adminRecords();
   for(const n of nodes){const view=cardViews.get(n.id);if(view)patchCard(view,n,records?.get(n.id));}
  }else render();
 }
 if(state.dialogId){
  const current=nodes.find(node=>node.id===state.dialogId);
  if(current)showDetail(current);else $('#node-dialog').close();
 }
}
admin=initAdmin({el,icon,flag,setData:applyData,setSite:applySite,onBoardOpen:render,onLogout:()=>{$('#node-dialog').close();$('#dialog-content').replaceChildren();nodes.splice(0);render();},onLoadError:(message,denied)=>{if(!denied&&nodes.length){setText($('.demo-label'),'重新连接中');return;}$('#node-dialog').close();$('#dialog-content').replaceChildren();nodes.splice(0);syncCards([]);$('#server-grid').hidden=true;$('#empty-state').hidden=false;$('#empty-description').textContent=message;$('.demo-label').textContent='连接中断';}});
render();
let pollTimer, pollGeneration=0;
function schedulePoll(generation,delay){
 pollTimer=setTimeout(async()=>{
  if(generation!==pollGeneration||document.hidden)return;
  const start=performance.now();
  await admin.refresh(true);
  if(generation!==pollGeneration)return;
  const period=$('#main-content').hidden?4000:1000;
  schedulePoll(generation,Math.max(250,period-(performance.now()-start)));
 },delay);
}
function restartPolling(){
 clearTimeout(pollTimer);
 const generation=++pollGeneration;
 admin.cancelBoardRead();
 for(const view of visibleCards){view.sampleAt=0;view.motion?.cancel();view.motion=null;}
 if(!document.hidden)schedulePoll(generation,0);
}
document.addEventListener('visibilitychange',restartPolling);
window.addEventListener('pageshow',event=>{if(event.persisted)restartPolling();});
schedulePoll(pollGeneration,1000);
