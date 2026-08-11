//! Standalone, lazy-loaded asset time-series view.

pub fn page() -> &'static str {
    PAGE
}

const PAGE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1">
  <title>Asset Comparison</title>
  <style>
    *{box-sizing:border-box} body{margin:0;padding:18px;background:#f3f3f3;color:#111;font:15px/1.4 "Courier New",monospace}
    main{max-width:1800px;margin:auto;border:2px solid #222;background:#f7f7f7}
    header,.section{padding:16px 20px;border-bottom:2px solid #222} header{display:flex;justify-content:space-between;align-items:center}
    h1,h2{margin:0;font-size:22px}.brand{color:#6f2da8;font-size:27px;font-weight:700}
    .table-scroll{overflow-x:auto} table{width:100%;border-collapse:collapse;margin-top:12px} th,td{padding:5px 18px 5px 0;white-space:nowrap;text-align:right}
    th:first-child,td:first-child{text-align:left} th{border-bottom:1px dashed #777} th[data-sort]{cursor:pointer;user-select:none} th.sort-asc::after{content:" ▲"} th.sort-desc::after{content:" ▼"} tbody tr{cursor:pointer} tbody tr:hover,tbody tr.selected{background:#dedede;font-weight:700}
    .charts{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:16px;padding:16px 20px}.chart{border:1px solid #222;padding:12px;min-width:0}.chart h2{font-size:17px;margin-bottom:3px}.chart p{margin:0 0 9px;color:#555}
    canvas{display:block;width:100%;height:310px;cursor:grab;touch-action:none}canvas.dragging{cursor:grabbing}.muted{color:#666}.error{color:#b00020}.cost-good{color:#14833b}.cost-bad{color:#b00020}
    .zoom-controls,.asset-actions{display:flex;gap:8px;align-items:center;justify-content:space-between}.zoom-controls button,.asset-actions button{border:1px solid #222;background:#f7f7f7;font:inherit;padding:3px 10px;cursor:pointer}
    @media(max-width:760px){.charts{grid-template-columns:1fr}}
  </style>
</head>
<body><main>
  <header><div><h1>ASSET COMPARISON</h1><div id="strategy" class="muted"></div></div><div class="brand">VPS Securities JSC</div></header>
  <section class="section"><div class="asset-actions"><span>Click an asset to select or unselect its session charts.</span><button id="unselect-all">Unselect All</button></div><div class="table-scroll"><table><thead><tr><th data-sort="asset">Asset</th><th data-sort="runs">Runs</th><th data-sort="mean_cost">Mean IS %</th><th data-sort="p50_cost">P50 IS %</th><th data-sort="avg_mean_divergence">Avg Mean Divergence</th><th data-sort="avg_max_divergence">Avg Max Divergence</th><th data-sort="completion">Completion</th><th data-sort="fees">Fees</th><th data-sort="status">Status</th></tr></thead><tbody id="assets"></tbody></table></div></section>
  <div id="message" class="section muted">No asset selected.</div>
  <div id="zoom-controls" class="section zoom-controls" hidden><strong>TIME ZOOM:</strong><button id="zoom-in">+</button><button id="zoom-out">−</button><button id="zoom-reset">Reset</button><span>Mouse wheel also zooms all charts together.</span></div>
  <section id="charts" class="charts" hidden>
    <div class="chart"><h2>IMPLEMENTATION SHORTFALL %</h2><p>Total execution cost from filled cost, residual cost, and fees. The lower the better (negative means we beat the arrival mid price).</p><canvas id="is-chart"></canvas></div>
    <div class="chart"><h2>FILLED COST %</h2><p>Cost of completed fills relative to the arrival mid. The lower the better (negative means we beat the arrival mid price).</p><canvas id="filled-cost-chart"></canvas></div>
    <div class="chart"><h2>RESIDUAL COST %</h2><p>Cost of remaining inventory valued at the final mid. The lower the better (negative means we beat the arrival mid price).</p><canvas id="residual-cost-chart"></canvas></div>
    <div class="chart"><h2>FEES</h2><p>Total execution fees; negative values represent rebates.</p><canvas id="fees-chart"></canvas></div>
    <div class="chart"><h2>COMPLETION %</h2><p>Percentage of the initial position executed. The higher the better.</p><canvas id="completion-chart"></canvas></div>
  </section>
</main><script>
  const strategy = Number(new URLSearchParams(location.search).get('strategy') || 0);
  const sorting = {column:'asset',order:'asc'};
  const selectedSeries=new Map(),seriesCache=new Map(),palette=['#6f2da8','#0072b2','#d55e00','#009e73','#cc79a7','#e69f00','#56b4e9','#8b4513','#4b0082','#708090'];
  let chartPoints=[],fullDomain=null,viewDomain=null,hoverTime=null,dragState=null;
  const fmt = v => v == null || !Number.isFinite(v) ? '—' : v.toFixed(2);
  const pct = v => v == null || !Number.isFinite(v) ? '—' : v.toFixed(2)+'%';
  const et = ms => new Intl.DateTimeFormat('en-US',{timeZone:'America/New_York',month:'short',day:'2-digit',hour:'2-digit',minute:'2-digit'}).format(new Date(ms));
  async function json(url){const r=await fetch(url);if(!r.ok)throw new Error(await r.text());return r.json()}
  function td(row,text,cls=''){const c=document.createElement('td');c.textContent=text;c.className=cls;row.appendChild(c)}
  function costClass(v){return v<0?'cost-good':v>0?'cost-bad':''}

  function draw(id, key, suffix='') {
    const canvas=document.getElementById(id), rect=canvas.getBoundingClientRect(), dpr=devicePixelRatio||1;
    canvas.width=Math.max(1,rect.width*dpr);canvas.height=Math.max(1,rect.height*dpr);
    const c=canvas.getContext('2d');c.scale(dpr,dpr);const w=rect.width,h=rect.height,p={l:70,r:20,t:15,b:42};
    const [xmin,xmax]=viewDomain||[0,1],valid=chartPoints.filter(x=>x.time_ms>=xmin&&x.time_ms<=xmax&&Number.isFinite(x[key]));c.font='12px Courier New';c.strokeStyle='#222';c.fillStyle='#333';
    if(!valid.length){c.fillText('No valid data',p.l,p.t+20);return}
    valid.sort((a,b)=>a.time_ms-b.time_ms);
    let ymin=Math.min(0,...valid.map(x=>x[key])),ymax=Math.max(0,...valid.map(x=>x[key]));if(ymin===ymax){ymin-=1;ymax+=1}
    const x=v=>p.l+(v-xmin)/(xmax-xmin||1)*(w-p.l-p.r), y=v=>p.t+(ymax-v)/(ymax-ymin)*(h-p.t-p.b);
    c.beginPath();c.moveTo(p.l,p.t);c.lineTo(p.l,h-p.b);c.lineTo(w-p.r,h-p.b);c.stroke();
    for(let i=0;i<=4;i++){const value=ymin+(ymax-ymin)*i/4,py=y(value);c.strokeStyle='#ddd';c.beginPath();c.moveTo(p.l,py);c.lineTo(w-p.r,py);c.stroke();c.fillStyle='#333';c.fillText(value.toFixed(2)+suffix,4,py+4)}
    c.strokeStyle='#555';c.setLineDash([5,4]);c.beginPath();c.moveTo(p.l,y(0));c.lineTo(w-p.r,y(0));c.stroke();c.setLineDash([]);
    const groups=new Map();valid.forEach(v=>{if(!groups.has(v._asset))groups.set(v._asset,[]);groups.get(v._asset).push(v)});groups.forEach(points=>{c.strokeStyle=points[0]._color;c.lineWidth=1.7;c.beginPath();points.forEach((v,i)=>i?c.lineTo(x(v.time_ms),y(v[key])):c.moveTo(x(v.time_ms),y(v[key])));c.stroke()});
    c.fillStyle='#333';c.fillText(et(xmin),p.l,h-12);const end=et(xmax);c.fillText(end,w-p.r-c.measureText(end).width,h-12);
    if(hoverTime!=null){const hovered=[];groups.forEach(points=>{const point=points.reduce((best,v)=>Math.abs(v.time_ms-hoverTime)<Math.abs(best.time_ms-hoverTime)?v:best,points[0]);hovered.push(point)});hovered.sort((a,b)=>a._asset.localeCompare(b._asset));const cursorX=x(hoverTime);c.strokeStyle='#777';c.beginPath();c.moveTo(cursorX,p.t);c.lineTo(cursorX,h-p.b);c.stroke();c.font='bold 15px Courier New';const labels=hovered.map(point=>`${point._asset}  ${et(point.time_ms)} ET  ${point[key].toFixed(4)}${suffix}`),boxWidth=Math.max(...labels.map(label=>c.measureText(label).width))+34,boxHeight=labels.length*23+10,boxX=Math.min(Math.max(p.l,cursorX-boxWidth/2),w-p.r-boxWidth),boxY=p.t; c.fillStyle='#fff';c.fillRect(boxX,boxY,boxWidth,boxHeight);c.strokeStyle='#222';c.strokeRect(boxX,boxY,boxWidth,boxHeight);hovered.forEach((point,i)=>{const px=x(point.time_ms),py=y(point[key]),lineY=boxY+21+i*23;c.fillStyle=point._color;c.beginPath();c.arc(px,py,5,0,Math.PI*2);c.fill();c.fillRect(boxX+7,lineY-11,10,10);c.fillStyle='#111';c.fillText(labels[i],boxX+24,lineY)})}
  }
  function renderCharts(){draw('is-chart','is_pct','%');draw('filled-cost-chart','filled_cost_pct','%');draw('residual-cost-chart','residual_cost_pct','%');draw('completion-chart','completion_pct','%');draw('fees-chart','fees')}
  function zoom(factor,center){if(!viewDomain||!fullDomain)return;const [a,b]=viewDomain,c=center??(a+b)/2,minSpan=Math.max(1,(fullDomain[1]-fullDomain[0])/1000),span=Math.max(minSpan,Math.min(fullDomain[1]-fullDomain[0],(b-a)*factor));let left=c-(c-a)/(b-a||1)*span;left=Math.max(fullDomain[0],Math.min(left,fullDomain[1]-span));viewDomain=[left,left+span];renderCharts()}
  document.querySelectorAll('canvas').forEach(canvas=>{canvas.addEventListener('mousemove',e=>{if(!viewDomain||dragState)return;const r=canvas.getBoundingClientRect(),left=70,right=20;hoverTime=viewDomain[0]+Math.max(0,Math.min(1,(e.clientX-r.left-left)/(r.width-left-right)))*(viewDomain[1]-viewDomain[0]);renderCharts()});canvas.addEventListener('mouseleave',()=>{hoverTime=null;renderCharts()});canvas.addEventListener('wheel',e=>{e.preventDefault();const r=canvas.getBoundingClientRect(),ratio=Math.max(0,Math.min(1,(e.clientX-r.left-70)/(r.width-90))),center=viewDomain[0]+ratio*(viewDomain[1]-viewDomain[0]);zoom(e.deltaY<0?.936:1.064,center)},{passive:false})});
  document.querySelectorAll('canvas').forEach(canvas=>{canvas.addEventListener('pointerdown',e=>{if(!viewDomain||!fullDomain)return;dragState={x:e.clientX,domain:[...viewDomain],width:Math.max(1,canvas.getBoundingClientRect().width-90)};canvas.setPointerCapture(e.pointerId);canvas.classList.add('dragging')});canvas.addEventListener('pointermove',e=>{if(!dragState)return;const span=dragState.domain[1]-dragState.domain[0],fullSpan=fullDomain[1]-fullDomain[0];if(span>=fullSpan)return;let left=dragState.domain[0]-(e.clientX-dragState.x)/dragState.width*span;left=Math.max(fullDomain[0],Math.min(left,fullDomain[1]-span));viewDomain=[left,left+span];hoverTime=null;renderCharts()});const stop=e=>{if(dragState){dragState=null;canvas.classList.remove('dragging');if(canvas.hasPointerCapture(e.pointerId))canvas.releasePointerCapture(e.pointerId)}};canvas.addEventListener('pointerup',stop);canvas.addEventListener('pointercancel',stop)});
  document.getElementById('zoom-in').onclick=()=>zoom(.8);document.getElementById('zoom-out').onclick=()=>zoom(1.25);document.getElementById('zoom-reset').onclick=()=>{viewDomain=[...fullDomain];renderCharts()};window.addEventListener('resize',renderCharts);
  function rebuildCharts(){chartPoints=[];selectedSeries.forEach(series=>series.points.forEach(p=>{if(Number.isFinite(p.time_ms))chartPoints.push({...p,_asset:series.name,_color:series.color})}));chartPoints.sort((a,b)=>a.time_ms-b.time_ms);fullDomain=chartPoints.length?[chartPoints[0].time_ms,chartPoints.at(-1).time_ms]:null;viewDomain=fullDomain?[...fullDomain]:null;hoverTime=null;const names=[...selectedSeries.values()].map(s=>s.name);document.getElementById('message').textContent=names.length?`Selected: ${names.join(', ')} — time shown in ET`:'No asset selected.';document.getElementById('charts').hidden=!names.length;document.getElementById('zoom-controls').hidden=!names.length;renderCharts()}
  async function selectAsset(index,name,row){
    if(selectedSeries.has(index)){selectedSeries.delete(index);row.classList.remove('selected');rebuildCharts();return}
    const message=document.getElementById('message');message.textContent=`Loading ${name}…`;
    try {let data=seriesCache.get(index);if(!data){data=await json(`/api/asset-series?strategy=${strategy}&asset=${index}`);seriesCache.set(index,data)}selectedSeries.set(index,{name,color:palette[index%palette.length],points:data.points});row.classList.add('selected');rebuildCharts();
    } catch(e){message.textContent=`Load failed: ${e.message}`;message.className='section error'}
  }
  function showSort(){document.querySelectorAll('th[data-sort]').forEach(h=>{h.classList.remove('sort-asc','sort-desc');if(h.dataset.sort===sorting.column)h.classList.add(`sort-${sorting.order}`)})}
  async function loadAssets(){const assets=await json(`/api/assets?strategy=${strategy}&page=1&size=50&sort=${sorting.column}&order=${sorting.order}`),body=document.getElementById('assets');body.replaceChildren();assets.items.forEach(a=>{const row=document.createElement('tr'),name=`${a.timeframe}/${a.asset.toUpperCase()}`;if(selectedSeries.has(a.index))row.classList.add('selected');const nameCell=document.createElement('td'),dot=document.createElement('span');dot.textContent='● ';dot.style.color=palette[a.index%palette.length];nameCell.append(dot,document.createTextNode(name));row.appendChild(nameCell);td(row,String(a.stats.runs));td(row,pct(a.stats.mean_cost),costClass(a.stats.mean_cost));td(row,pct(a.stats.p50_cost),costClass(a.stats.p50_cost));td(row,fmt(a.stats.avg_mean_divergence));td(row,fmt(a.stats.avg_max_divergence));td(row,pct(a.stats.completion_pct));td(row,fmt(a.stats.fees));td(row,pct(a.stats.status_pct));row.onclick=()=>selectAsset(a.index,name,row);body.appendChild(row)})}
  document.querySelectorAll('th[data-sort]').forEach(h=>h.onclick=async()=>{const same=sorting.column===h.dataset.sort;sorting.order=same&&sorting.order==='asc'?'desc':'asc';sorting.column=h.dataset.sort;showSort();await loadAssets()});showSort();
  document.getElementById('unselect-all').onclick=()=>{selectedSeries.clear();document.querySelectorAll('#assets tr').forEach(row=>row.classList.remove('selected'));rebuildCharts()};
  async function start(){try{const source=await json(`/api/source?strategy=${strategy}`);document.title=`Asset Comparison — ${source.name}`;document.getElementById('strategy').textContent=source.name;await loadAssets()
  }catch(e){document.getElementById('message').textContent=`Load failed: ${e.message}`;document.getElementById('message').className='section error'}} start();
</script></body></html>"#;
