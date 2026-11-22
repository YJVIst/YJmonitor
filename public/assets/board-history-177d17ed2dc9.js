// Agents retain 40 CPU samples. A fixed horizontal scale keeps the graph steady
// while a newly connected node is still filling its two-minute window.
const capacity=40, width=112, height=42, step=108/(capacity-1);

export function historyTransition(previous, samples, continuous, remaining=0){
 const history=samples.slice(-capacity);
 let overlap=Math.min(previous.length,history.length);
 while(overlap&&!previous.slice(-overlap).every((value,i)=>value===history[i]))overlap--;
 const added=history.length-overlap;
 const scroll=continuous&&overlap>0&&added>0&&added<=2;
 return {history,points:scroll?[...previous,...history.slice(overlap)]:history,
  offset:scroll?remaining+added*step/width*100:0};
}

export function historyPath(points){
 const coordinates=points.map((value,index)=>{
  const x=110-(points.length-1-index)*step;
  const y=height-3-Math.min(100,Math.max(0,value))*(height-6)/100;
  return x.toFixed(2)+' '+y.toFixed(2);
 });
 if(!coordinates.length)return {line:'',fill:''};
 const line='M'+coordinates.join(' L');
 const left=110-(points.length-1)*step;
 return {line,fill:line+' L110 42 L'+left.toFixed(2)+' 42 Z'};
}
