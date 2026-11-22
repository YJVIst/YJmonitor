const byteFormats=[0,2].map(maximumFractionDigits=>new Intl.NumberFormat('zh-CN',{maximumFractionDigits}));
export function formatBytes(value){
 if(typeof value!=='number'||!Number.isFinite(value)||value<0)return '—';
 const units=['B','KiB','MiB','GiB','TiB','PiB'];let i=0;
 while(value>=1024&&i<units.length-1){value/=1024;i++;}
 return byteFormats[i?1:0].format(value)+' '+units[i];
}
export function createNetworkUI({el}){
 const choices=new Map();
 function pair(label,value,cls=''){const d=el('dl',cls);d.append(el('dt','',label),el('dd','',value));return d;}
 const stateName={up:'已连接',down:'未连接',unknown:'可用',dormant:'休眠',lowerlayerdown:'链路断开',notpresent:'未就绪',testing:'检测中'};
 function values(nic,online,compact){
  const box=el('div','network-values'+(compact?' compact':''));
  for(const [label,key]of [['↓ 下载','rx'],['↑ 上传','tx']]){
   const column=el('div','network-direction');const value=nic&&online&&nic[key+'_rate']!==null?formatBytes(nic[key+'_rate'])+'/s':'—';
   column.append(pair(label,value,'network-rate'),pair('累计',nic?formatBytes(nic[key+'_bytes']):'—','network-total'));box.append(column);
  }
  return box;
 }
 const states=new WeakMap();
 function card(node){
  const box=el('section','network-block'),heading=el('div','network-heading');
  box.setAttribute('aria-label','网卡流量');heading.append(el('span','metric-label','网络'));box.append(heading);
  const select=el('select','network-selector'),empty=el('span','network-empty','暂无网卡数据');
  select.dataset.node=node.id;select.setAttribute('aria-label',node.name+' 网卡');heading.append(select,empty);
  const metrics=values(null,false,true);box.append(metrics);
  const state={select,empty,metrics,fields:[...metrics.querySelectorAll('dd')],node};states.set(box,state);
  select.addEventListener('change',()=>{choices.set(state.node.id,select.value);updateCard(box,state.node);});
  updateCard(box,node);return box;
 }
 function updateCard(box,node){
  const s=states.get(box);s.node=node;
  const interfaces=node.networkAvailable&&Array.isArray(node.network)?node.network:[];
  const previous=s.interfaces;
  if(!previous||interfaces.length!==previous.length||interfaces.some((nic,i)=>nic.name!==previous[i].name||!!nic.default!==!!previous[i].default)){
   s.select.replaceChildren(...interfaces.map(n=>new Option(n.name+(n.default?' · 默认':''),n.name)));
   s.select.hidden=s.metrics.hidden=!interfaces.length;s.empty.hidden=!!interfaces.length;
  }
  s.interfaces=interfaces;
  if(!interfaces.length){s.nic=null;return;}
  const nic=interfaces.find(n=>n.name===choices.get(node.id))||interfaces.find(n=>n.default)||interfaces[0];
  if(s.select.value!==nic.name)s.select.value=nic.name;
  const old=s.nic, onlineChanged=s.online!==node.online;
  for(const [i,key] of ['rx_rate','rx_bytes','tx_rate','tx_bytes'].entries()){
   const rate=i===0||i===2;
   if(old&&old[key]===nic[key]&&(!rate||!onlineChanged))continue;
   const value=rate?(node.online&&Number.isFinite(nic[key])?formatBytes(nic[key])+'/s':'—'):formatBytes(nic[key]);
   if(s.fields[i].textContent!==value)s.fields[i].textContent=value;
  }
  s.nic=nic;s.online=node.online;
 }
 function detail(node){
  const section=el('section','detail-network');const heading=el('div','section-heading');heading.append(el('h3','','网卡流量'),el('span','',node.online?'实时采样':'最后一次上报'));section.append(heading);
  const interfaces=node.networkAvailable&&Array.isArray(node.network)?node.network:[];
  if(!interfaces.length){section.append(el('p','network-empty','暂无网卡数据'));return section;}
  const list=el('div','network-interface-list');
  for(const nic of interfaces){const row=el('article','network-interface');const top=el('div','network-interface-heading');top.append(el('strong','',nic.name));const tags=el('span','network-interface-tags');if(nic.default)tags.append(el('span','','默认路由'));if(nic.virtual)tags.append(el('span','','虚拟网卡'));tags.append(el('span','',node.online?(stateName[nic.state]||nic.state):'离线'));top.append(tags);row.append(top,values(nic,node.online,false));list.append(row);}
  section.append(list,el('p','form-footnote','累计值为网卡收发计数，系统重启或网卡重建后可能重置。各网卡分别展示，避免桥接、隧道流量重复相加。'));return section;
 }
 return {card,updateCard,detail};
}
