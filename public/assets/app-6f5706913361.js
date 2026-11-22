import {createPager} from "/assets/pagination-d84897e63d5d.js";
import {refreshAppearance} from "/assets/appearance-40485caa5df3.js";
import {initAdmin} from "/assets/admin-e0effea2f814.js";
import {countryByCode,normalizedCountry} from "/assets/countries-3dabb5bbec57.js";
import {createNetworkUI} from "/assets/network-9642a48fbf95.js";

const $=selector=>document.querySelector(selector);
const NS='http://www.w3.org/2000/svg';
const format=new Intl.NumberFormat('zh-CN',{maximumFractionDigits:2});
const clock=new Intl.DateTimeFormat('zh-CN',{hour:'2-digit',minute:'2-digit',second:'2-digit',hour12:false});
const nodes=[];
let admin=null;
let demoMode=false;
const baseCPU=new Map();
const state={status:'all',country:'all',group:'all',query:'',sort:'default',running:true,step:0,updated:new Date(),dialogId:null,refreshSeconds:5};
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
 pause:['M8 5v14M16 5v14'],play:['m8 5 11 7-11 7V5z'],
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
function sparkline(history,large=false){const w=large?500:112,h=large?100:42,s=svgEl('svg',{viewBox:`0 0 ${w} ${h}`,preserveAspectRatio:'none',class:large?'':'sparkline','aria-hidden':'true'});if(!history.length)return s;if(history.length===1)history=[history[0],history[0]];const coordinates=history.map((v,i)=>[2+i*(w-4)/(history.length-1),h-3-Math.min(100,Math.max(0,v))*(h-6)/100]);const line=coordinates.map(([x,y],i)=>`${i?'L':'M'}${x.toFixed(2)} ${y.toFixed(2)}`).join(' ');s.append(svgEl('path',{d:`${line} L${w-2} ${h} L2 ${h} Z`,class:'spark-fill'}),svgEl('path',{d:line,class:'spark-line'}));if(history.at(-1)>=85&&!large)s.classList.add('warning');return s;}
function resource(label,kind,values,online=true){const box=el('div','resource-block');const heading=el('span','metric-label');heading.append(icon(kind),document.createTextNode(label));const value=el('div','resource-value');value.append(document.createTextNode(online?display(values?.used):'—'),el('span','resource-total',values?.total>0?'/ '+display(values.total)+' GiB':''));const ratio=online&&values?percent(values.used,values.total):null;box.append(heading,value,meter(ratio,label+'使用率'));const caption=el('div','resource-caption');caption.append(el('span','',finite(ratio)?ratio.toFixed(1)+'%':online?'未启用':'暂无数据'),el('span','','已用 / 总量'));box.append(caption);return box;}
function openButton(node,cls,text){const b=el('button',cls,text);b.type='button';b.dataset.node=node.id;b.setAttribute('aria-label',`查看 ${node.name} 详情`);b.addEventListener('click',()=>openDetail(node.id));return b;}
function makeCard(node){const article=el('article','server-card'+(node.online?'':' is-offline'));article.dataset.id=node.id;article.setAttribute('aria-labelledby','title-'+node.id);const heading=el('div','card-heading');const flagWrap=el('span','flag-wrap');flagWrap.append(flag(node.code));const identity=el('div','node-identity');const title=el('h2','node-title');title.id='title-'+node.id;title.append(openButton(node,'',node.name));identity.append(title,el('p','node-location',locationLabel(node)));if(node.group||node.pinned)identity.append(el('p','node-group',(node.pinned?'置顶 · ':'')+(node.group||'未分组')));const badges=el('div','node-badges');badges.append(statusBadge(node),el('span','latency-badge'));heading.append(flagWrap,identity,badges);const hardware=el('div','hardware-row');const model=el('span','cpu-model',node.cpuModel);model.title=node.cpuModel;hardware.append(icon('cpu'),model,el('span','core-count',(node.cores||'—')+' vCPU'));const content=el('div','card-metrics');const footer=el('div','card-footer');const up=el('span','uptime');up.append(icon('clock'),document.createTextNode(node.online?'运行 '+node.uptime:(node.pending?'等待首次上报':'最后上报 '+node.lastSeenMinutes+' 分钟前')));const detail=openButton(node,'detail-link','详情');detail.append(icon('arrow'));footer.append(up,detail);article.append(heading,hardware,content,footer);patchLatencyBadge(article.querySelector('.latency-badge'),node);admin?.decorateCard(article,node);return article;}
function latencyReading(node){return node?.online&&finite(node.latencyMs)&&node.latencyMs>=0?Math.round(node.latencyMs)+' ms':'—';}
function patchLatencyBadge(badge,node){
 if(!badge)return;
 const value=latencyReading(node);
 badge.textContent='通讯 '+value;
 badge.title='Agent 与主控通过 WSS 实测的往返延时';
 badge.setAttribute('aria-label',value==='—'?'与主控的通讯延时暂无数据':'与主控的通讯延时：'+value);
 badge.classList.toggle('is-muted',value==='—');
}
const cardViews=new Map();
const visibleCards=new Set();
const reducedMotion=matchMedia('(prefers-reduced-motion: reduce)');
const counters=new Map();
let counterFrame=0,lastCounterPaint=0;
function paintCounters(at){
 counterFrame=0;
 if(document.hidden||!state.running){counters.clear();return;}
 if(at-lastCounterPaint>=50){
  lastCounterPaint=at;
  for(const [view,run] of counters){
   if(!visibleCards.has(view)){counters.delete(view);continue;}
   const t=Math.max(0,Math.min(1,(at-run.start)/run.duration));
   const cpu=run.from+(run.to-run.from)*t;
   view.currentCPU=cpu;setText(view.number,cpu.toFixed(1));
   if(t===1)counters.delete(view);
  }
 }
 if(counters.size)counterFrame=requestAnimationFrame(paintCounters);
}
function updateCPU(view,cpu,ms){
 if(!finite(cpu)||!ms||!finite(view.currentCPU)||reducedMotion.matches){
  counters.delete(view);view.currentCPU=cpu;setText(view.number,finite(cpu)?cpu.toFixed(1):'—');return;
 }
 if(view.currentCPU===cpu)return;
 counters.set(view,{from:view.currentCPU,to:cpu,start:performance.now(),duration:ms});
 if(!counterFrame)counterFrame=requestAnimationFrame(paintCounters);
}
const nearViewport=new IntersectionObserver(entries=>{
 for(const {target,isIntersecting} of entries){
  const view=cardViews.get(target.dataset.id);
  if(view?.card!==target)continue;
  if(isIntersecting){visibleCards.add(view);updateMetrics(view);}
  else{visibleCards.delete(view);counters.delete(view);view.motion?.cancel();view.motion=null;}
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
function plotHistory(view,samples,ms){
 const history=samples.slice(-40),prev=view.history||[];
 if(history.length===prev.length&&history.every((v,i)=>v===prev[i]))return;
 view.history=history;view.motion?.cancel();view.motion=null;
 if(!history.length){view.strip.replaceChildren();return;}
 let overlap=Math.min(prev.length,history.length);
 while(overlap&&!prev.slice(-overlap).every((v,i)=>v===history[i]))overlap--;
 const shift=prev.length===history.length&&ms>0?history.length-overlap:0;
 const series=shift?[...prev,...history.slice(overlap)]:history;
 const width=100*(series.length-1)/Math.max(history.length-1,1)||100;
 view.strip.style.width=width+'%';
 const svg=sparkline(series);svg.classList.remove('sparkline');view.strip.replaceChildren(svg);
 const end=(100-width)/width*100;
 view.strip.style.transform='translateX('+end+'%)';
 if(shift)view.motion=view.strip.animate([{transform:'translateX(0)'},{transform:'translateX('+end+'%)'}],{duration:ms,easing:'linear'});
}
function updateMetrics(view){
 const n=view.node;
 if(!view.resources)buildMetrics(view);
 const ms=view.sampleAt&&n.online&&state.running&&!reducedMotion.matches?Math.min(state.refreshSeconds*1000,Math.max(200,performance.now()-view.sampleAt)):0;
 view.sampleAt=performance.now();
 updateCPU(view,n.online?n.cpu:null,ms);
 view.value.classList.toggle('warning',n.cpu>=85);
 if(n.online)plotHistory(view,n.history,ms);
 const disk=diskTotal(n);
 for(const [j,values] of [n.memory,disk].entries()){
  const r=view.resources[j],used=n.online?values?.used:null,total=values?.total,ratio=percent(used,total),value=display(used);
  if(r.value.data!==value)r.value.data=value;
  setText(r.total,total>0?'/ '+display(total)+' GiB':'');
  setText(r.ratio,ratio!==null?ratio.toFixed(1)+'%':n.online?'未启用':'暂无数据');
  r.bar.style.transitionDuration=ms+'ms';r.bar.style.transform='scaleX('+((ratio||0)/100)+')';
  r.track.classList.toggle('warning',ratio>=85);
  if(ratio===null)r.track.removeAttribute('aria-valuenow');else r.track.setAttribute('aria-valuenow',ratio.toFixed(1));
 }
 if(view.swap){setText(view.swap,n.online?display(n.swap.used)+' / '+display(n.swap.total)+' GiB':'暂无数据');view.swap.classList.toggle('swap-warning',n.online&&percent(n.swap.used,n.swap.total)>=85);}
 networkUI.updateCard(view.net,n);
}
function syncCards(list){
 const grid=$('#server-grid'),wanted=new Set(list.map(n=>n.id));
 for(const [id,v] of cardViews)if(!wanted.has(id)){nearViewport.unobserve(v.card);visibleCards.delete(v);counters.delete(v);v.motion?.cancel();v.card.remove();cardViews.delete(id);}
 grid.classList.toggle('many-nodes',list.length>8);
 const isAdmin=admin?.isAdmin();
 const records=isAdmin?new Map(admin.getRecords().map(r=>[r.public.id,r])):null;
 let cursor=grid.firstElementChild;
 for(const n of list){
  const record=records?.get(n.id);
  const signature=JSON.stringify([n.name,n.country,n.city,n.code,n.group,n.pinned,n.online,n.pending,n.cpuModel,n.cores,hasSwap(n),isAdmin]);
  let v=cardViews.get(n.id);
  if(!v||v.signature!==signature){
   const card=makeCard(n);
   if(v){if(cursor===v.card)cursor=card;nearViewport.unobserve(v.card);visibleCards.delete(v);counters.delete(v);v.motion?.cancel();v.card.replaceWith(card);}
   v={node:n,card,signature};cardViews.set(n.id,v);nearViewport.observe(card);
  }
  if(v.card!==cursor)grid.insertBefore(v.card,cursor);
  cursor=v.card.nextElementSibling;
  if(record){const ip=v.card.querySelector('.admin-ip-row code');if(ip)setText(ip,record.currentIP);}
  if(v.node!==n){
   v.node=n;patchLatencyBadge(v.card.querySelector('.latency-badge'),n);
   const uptime=v.card.querySelector('.uptime').lastChild,txt=n.online?'运行 '+n.uptime:(n.pending?'等待首次上报':'最后上报 '+n.lastSeenMinutes+' 分钟前');
   if(uptime.data!==txt)uptime.data=txt;
   if(visibleCards.has(v))updateMetrics(v);
  }
 }
}
function filtered(){const q=state.query.normalize('NFKC').toLocaleLowerCase();const list=nodes.filter(n=>(state.status==='all'||(state.status==='online')===n.online)&&(state.country==='all'||n.country===state.country)&&(state.group==='all'||'g:'+n.group===state.group)&&(!q||[n.name,n.group,n.country,n.city,n.cpuModel,n.arch].join(' ').normalize('NFKC').toLocaleLowerCase().includes(q)));if(state.sort==='cpu')list.sort((a,b)=>(b.online?b.cpu:-1)-(a.online?a.cpu:-1));if(state.sort==='memory')list.sort((a,b)=>(b.online?percent(b.memory.used,b.memory.total):-1)-(a.online?percent(a.memory.used,a.memory.total):-1));if(state.sort==='name')list.sort((a,b)=>a.name.localeCompare(b.name));if(state.sort==='default')list.sort((a,b)=>Number(!!b.pinned)-Number(!!a.pinned)||(a.order||0)-(b.order||0));return list;}
const boardPager=createPager(el,()=>{render();$('#server-grid').scrollIntoView({block:'start'});},60,'服务器看板');
$('#server-grid').after(boardPager.root);
function render(){const focused=document.activeElement;const focusedId=focused?.dataset?.node;const focusedClass=focused?.className;const list=filtered(),range=boardPager.range(list.length);syncCards(list.slice(range.start,range.end));$('#empty-state').hidden=list.length!==0;$('#server-grid').hidden=list.length===0;$('#result-count').textContent=list.length===nodes.length?'全部 '+list.length+' 台服务器':'显示 '+list.length+' / '+nodes.length+' 台服务器';$('#empty-description').textContent=state.query?'未找到与“'+state.query+'”匹配的节点。':'这个地区暂时没有符合条件的服务器。';for(const b of document.querySelectorAll('[data-status]')){const active=b.dataset.status===state.status;b.classList.toggle('active',active);b.setAttribute('aria-pressed',String(active));}if(focusedId){const restore=Array.from(document.querySelectorAll('[data-node]')).find(b=>b.dataset.node===focusedId&&b.className===focusedClass);restore?.focus({preventScroll:true});}}
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
function updateRefresh(){const active=state.running;$('#refresh-toggle').setAttribute('aria-pressed',String(active));$('#refresh-toggle').setAttribute('aria-label',active?(demoMode?'暂停演示自动刷新':'暂停实时刷新'):(demoMode?'继续演示自动刷新':'继续实时刷新'));$('#refresh-label').textContent=active?(demoMode?'演示自动刷新':'实时刷新'):(demoMode?'演示已暂停':'已暂停刷新');$('.refresh-icon').replaceChildren(icon(active?'pause':'play'));$('#refresh-time').textContent=clock.format(state.updated);}
$('#refresh-toggle').addEventListener('click',()=>{state.running=!state.running;for(const v of visibleCards){if(v.motion){if(state.running)v.motion.play();else v.motion.pause();} }updateRefresh();});
function showDetail(node){const root=$('#dialog-content');const heading=el('div','dialog-header');const flagWrap=el('span','flag-wrap');flagWrap.append(flag(node.code));const titles=el('div');const title=el('h2','',node.name);title.id='dialog-title';titles.append(title,el('p','node-location',locationLabel(node)));heading.append(flagWrap,titles,statusBadge(node));const hardware=el('div','detail-hardware');for(const [label,value]of [['CPU 型号',node.cpuModel],['核心 / 架构',node.cores+' vCPU · '+node.arch],['操作系统',node.system],['主控通讯延时',latencyReading(node)],['运行时间',node.online?node.uptime:'离线 · 等待上报']]){const block=el('dl');block.append(el('dt','',label),el('dd','',value));if(label==='主控通讯延时')block.dataset.metric='latency';hardware.append(block);}const sections=[heading,hardware];if(node.online){const chart=el('section','detail-chart');const chartHeader=el('div','section-heading');chartHeader.append(el('h3','','CPU 使用率'),el('span','',node.cpu.toFixed(1)+(demoMode?'% · 演示趋势':'% · 实时采样')));const chartSvg=sparkline(node.history,true);chartSvg.removeAttribute('aria-hidden');chartSvg.setAttribute('role','img');chartSvg.setAttribute('aria-label',(demoMode?'CPU 使用率演示曲线，当前 ':'CPU 使用率曲线，当前 ')+node.cpu.toFixed(1)+'%');const axis=el('div','chart-axis');axis.append(el('span','','最近 2 分钟'),el('span','','现在'));chart.append(chartHeader,chartSvg,axis);sections.push(chart);}else{sections.push(el('p','dialog-notice','最后一次上报于 '+node.lastSeenMinutes+' 分钟前。节点离线后暂停显示使用率，避免将过期数据作为当前状态。'));}const resources=el('div','detail-resource-row');resources.append(resource('内存','memory',node.memory,node.online));if(hasSwap(node))resources.append(resource('Swap','swap',node.swap,node.online));sections.push(resources);const disks=el('section');const label=el('div','section-heading');label.append(el('h3','','磁盘'),el('span','',node.disks.length+' 个卷 · 容量单位 GiB'));const list=el('ul','disk-list');for(const disk of node.disks){const item=el('li','disk-item');const row=el('div','disk-row');row.append(el('strong','',disk.name),el('span','',(node.online?display(disk.used):'—')+' / '+display(disk.total)+' GiB'));item.append(row,meter(node.online?percent(disk.used,disk.total):null,disk.name+'使用率'));list.append(item);}disks.append(label,list);sections.push(disks);sections.push(networkUI.detail(node));root.replaceChildren(...sections);admin?.decorateDetail(root,node);}
let dialogTrigger=null;
function openDetail(id){const node=nodes.find(n=>n.id===id);if(!node)return;dialogTrigger={id,detail:document.activeElement?.classList.contains('detail-link')};state.dialogId=id;showDetail(node);$('#node-dialog').showModal();$('#dialog-close').focus();}
$('#dialog-close').addEventListener('click',()=>$('#node-dialog').close());
$('#node-dialog').addEventListener('close',()=>{state.dialogId=null;if(dialogTrigger){const card=Array.from(document.querySelectorAll('.server-card')).find(c=>c.dataset.id===dialogTrigger.id);card?.querySelector(dialogTrigger.detail?'.detail-link':'.node-title button')?.focus({preventScroll:true});dialogTrigger=null;}});
$('#node-dialog').addEventListener('keydown',event=>{if(event.key!=='Tab')return;const controls=Array.from(event.currentTarget.querySelectorAll('button:not([disabled]),a[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])')).filter(e=>e.getClientRects().length>0);const first=controls[0],last=controls.at(-1);if(!first){event.preventDefault();event.currentTarget.focus();return;}if(controls.length===1||(event.shiftKey&&document.activeElement===first)||(!event.shiftKey&&document.activeElement===last)){event.preventDefault();(event.shiftKey?last:first).focus();}});
$('#node-dialog').addEventListener('click',event=>{if(event.target!==$('#node-dialog'))return;const rect=event.target.getBoundingClientRect();if(event.clientX<rect.left||event.clientX>rect.right||event.clientY<rect.top||event.clientY>rect.bottom)event.target.close();});
document.addEventListener('keydown',event=>{if(event.key==='/'&&!event.ctrlKey&&!event.metaKey&&!event.altKey&&!$('#node-dialog').open&&!['INPUT','TEXTAREA','SELECT'].includes(document.activeElement.tagName)){event.preventDefault();$('#search').focus();}});
let appliedSiteName='';
function applySite(site){
 const name=typeof site?.name==='string'&&site.name.trim()?site.name:'服务器监控';
 state.refreshSeconds=site.refreshSeconds||5;if(name===appliedSiteName)return;appliedSiteName=name;
 for(const element of document.querySelectorAll('[data-site-name]'))element.textContent=name;
 const brand=$('.brand');brand.setAttribute('aria-label',name+' 首页');brand.title=name;
 document.title='服务器状态 · '+name;
 document.querySelector('meta[name="description"]').content=name+' 服务器状态看板。查看节点所在地区、CPU、内存、Swap、磁盘与网络流量。';
}
function applyData(list,site,preview,themeRevision){
 refreshAppearance(themeRevision);
 demoMode=preview;applySite(site);
 const oldById=new Map(nodes.map(n=>[n.id,n]));
 nodes.splice(0,nodes.length,...list.map(n=>{const old=oldById.get(n.id);const c=countryByCode.get(normalizedCountry(n.code));if(c)n.country=c.name;if(n.demo&&old){n.cpu=old.cpu;n.history=old.history;}if(!baseCPU.has(n.id)&&finite(n.cpu))baseCPU.set(n.id,n.cpu);return n;}));
 const online=nodes.filter(n=>n.online).length,offline=nodes.length-online;
 $('#node-total').textContent=nodes.length;$('#online-total').textContent=online;$('#offline-total').textContent=offline;
 for(const [key,count]of [['all',nodes.length],['online',online],['offline',offline]])document.querySelector('[data-status="'+key+'"] span').textContent=count;
 const select=$('#country-filter');const countries=[...new Set(nodes.map(n=>n.country))];select.replaceChildren();const first=el('option','','全部地区');first.value='all';select.append(first);for(const country of countries){const option=el('option','',country);option.value=country;select.append(option);}if(!countries.includes(state.country))state.country='all';select.value=state.country;
 const groups=[...new Set(nodes.map(n=>n.group||''))].sort();const gs=$('#group-filter');gs.replaceChildren(new Option('全部分组','all'));for(const g of groups)gs.append(new Option(g||'未分组','g:'+g));if(!groups.some(g=>'g:'+g===state.group))state.group='all';gs.value=state.group;
 $('.demo-label').replaceChildren(el('span'),document.createTextNode(preview?'界面预览':'实时监控'));
 $('.footer>span').replaceChildren(el('span','footer-dot'),document.createTextNode(preview?'演示数据 · 尚未接入 Agent':'监测数据由 Agent 实时上报'));
 if(!$('#main-content').hidden)render();updateRefresh();
 if(state.dialogId){
  const current=nodes.find(node=>node.id===state.dialogId);
  if(current)showDetail(current);else $('#node-dialog').close();
 }
}
admin=initAdmin({el,icon,flag,setData:applyData,setSite:applySite,onBoardOpen:render,onLogout:()=>{$('#node-dialog').close();$('#dialog-content').replaceChildren();nodes.splice(0);render();},onLoadError:message=>{$('#node-dialog').close();$('#dialog-content').replaceChildren();nodes.splice(0);syncCards([]);$('#server-grid').hidden=true;$('#empty-state').hidden=false;$('#empty-description').textContent=message;$('.demo-label').textContent='连接中断';}});
render();updateRefresh();
setInterval(()=>{if(!state.running||document.hidden||Date.now()-state.updated.getTime()<state.refreshSeconds*1000)return;state.step++;if(demoMode)nodes.forEach((node,i)=>{if(!node.online||!node.demo)return;const base=baseCPU.get(node.id)??node.cpu;node.cpu=Math.round(Math.max(0,Math.min(100,base+Math.sin(state.step*.7+i)*2.1))*10)/10;node.history=[...node.history.slice(1),node.cpu];});state.updated=new Date();updateRefresh();admin.refresh(true);},1000);
