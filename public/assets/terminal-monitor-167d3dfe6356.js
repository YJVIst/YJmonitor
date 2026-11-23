const NS = 'http://www.w3.org/2000/svg';
export function compactBytes(value) {
  if (!Number.isFinite(value) || value < 0) return '—';
  const units = ['B', 'K', 'M', 'G', 'T', 'P']; let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return (unit ? Number(value.toFixed(value >= 100 ? 0 : 1)) : Math.round(value)) + units[unit];
}
export function workspaceWidth(value, available) {
  const min = available <= 700 ? 150 : 220;
  const max = Math.max(min, Math.min(520, available - (available <= 700 ? 155 : 380)));
  return Math.max(min, Math.min(max, Number(value) || (available <= 700 ? 180 : 254)));
}
export function workspaceHeight(value, available) {
  const max = Math.max(150, available - 240);
  return Math.max(150, Math.min(max, Number(value) || 208));
}
function node(tag, cls, text) {
  const e = document.createElement(tag); if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text; return e;
}
function chart(label, colors) {
  const box = node('section', 'terminal-mini-chart');
  const head = node('div', 'terminal-chart-heading'), title = node('strong', '', label), value = node('span');
  const svg = document.createElementNS(NS, 'svg'); svg.setAttribute('viewBox', '0 0 220 65');
  svg.setAttribute('role', 'img'); svg.setAttribute('aria-label', label); svg.setAttribute('preserveAspectRatio', 'none');
  for (const y of [15, 35, 55]) {
    const line = document.createElementNS(NS, 'path'); line.setAttribute('d', 'M0 ' + y + ' H220');
    line.setAttribute('stroke', '#dfe6eb'); line.setAttribute('stroke-dasharray', '2 3'); svg.append(line);
  }
  const paths = colors.map(color => {
    const p = document.createElementNS(NS, 'path'); p.setAttribute('fill', 'none');
    p.setAttribute('stroke', color); p.setAttribute('stroke-width', '1.5'); p.setAttribute('stroke-linejoin', 'round');
    p.setAttribute('vector-effect', 'non-scaling-stroke'); svg.append(p); return p;
  });
  const axis = node('div', 'terminal-chart-axis'), scale = node('span'), time = node('span', '', '最近 2 分钟');
  axis.append(scale, time); head.append(title, value); box.append(head, svg, axis);
  let series = [], times = [], max = 1, displayedMax = 1, changedAt = 0, lastFrame = 0, format = String, previousLast = [];
  function draw(next, caption, formatter, timestamps, now) {
    value.textContent = caption; format = formatter;
    if (timestamps.at(-1) !== times.at(-1) || timestamps.length !== times.length) {
      previousLast = timestamps.length > 1 ? series.map(list => list.at(-1)) : []; changedAt = now;
    }
    series = next.map(list => list.slice()); times = timestamps.slice();
    max = Math.max(1, ...series.flat().filter(Number.isFinite)) * 1.15;
    if (!lastFrame) displayedMax = max;
  }
  function render(now, motion = true) {
    const delta = Math.max(0, Math.min(64, now - lastFrame)); lastFrame = now;
    displayedMax = motion ? displayedMax + (max - displayedMax) * (1 - Math.exp(-delta / 180)) : max;
    if (Math.abs(displayedMax - max) < .001) displayedMax = max;
    const scaleLabel = '0 — ' + format(displayedMax); if (scale.textContent !== scaleLabel) scale.textContent = scaleLabel;
    const progress = motion ? Math.min(1, Math.max(0, (now - changedAt) / 350)) : 1;
    const ease = 1 - (1 - progress) ** 3;
    paths.forEach((p, k) => {
      let d = '', gap = true; const list = series[k] || [];
      list.forEach((n, i) => {
        if (!Number.isFinite(n)) { gap = true; return; }
        const x = 220 - (now - times[i]) / 120000 * 220;
        if (x < -5) { gap = true; return; }
        const shown = i === list.length - 1 && Number.isFinite(previousLast[k]) ? previousLast[k] + (n - previousLast[k]) * ease : n;
        const y = Math.max(3, Math.min(60, 60 - shown / displayedMax * 55));
        d += (gap ? 'M' : 'L') + x.toFixed(3) + ' ' + y.toFixed(2) + ' '; gap = false;
      });
      if (p.getAttribute('d') !== d) p.setAttribute('d', d);
    });
  }
  return {box, head, draw, render};
}
function createProcessTable(holder, request, available) {
  const box = node('div', 'terminal-processes'), table = node('table'), head = node('thead'), headings = node('tr'), body = node('tbody');
  table.setAttribute('aria-label', '实时进程资源');
  const buttons = new Map();
  let started = false, busy = false, epoch = 0, at = 0, completedAt = 0, sort = 'cpu', records = [], timer, deferred;
  for (const [key, label] of [['memory', '内存'], ['cpu', 'CPU'], ['name', '命令']]) {
    const cell = node('th');
    if (key === 'name') cell.textContent = label;
    else {
      const button = node('button', '', label); button.type = 'button'; button.setAttribute('aria-label', '按' + label + '排序');
      button.onclick = () => { sort = key; render(); clearTimeout(deferred); deferred = setTimeout(load, Math.max(0, 1100 - (Date.now() - completedAt))); };
      cell.append(button); buttons.set(key, {button, cell});
    }
    headings.append(cell);
  }
  head.append(headings); table.append(head, body); box.append(table); holder.append(box);
  function note(text) {
    const row = node('tr'), cell = node('td', 'terminal-process-note', text); cell.colSpan = 3; row.append(cell); body.replaceChildren(row);
  }
  function render() {
    for (const [key, item] of buttons) {
      item.cell.setAttribute('aria-sort', key === sort ? 'descending' : 'none');
      item.button.classList.toggle('active', key === sort);
    }
    const scroll = box.scrollTop; body.replaceChildren();
    const rows = records.slice().sort((a,b) => (b[sort] || 0) - (a[sort] || 0) || b.memory - a.memory).slice(0,10);
    for (const process of rows) {
      const row = node('tr'), command = node('td', 'terminal-process-command', process.name);
      command.title = process.name + ' · PID ' + process.pid;
      const cpu = node('td', '', Number.isFinite(process.cpu) ? Number(process.cpu.toFixed(1)) + '%' : '—'); cpu.title = 'CPU 按单核 100% 计';
      row.append(node('td', '', compactBytes(process.memory)), cpu, command); body.append(row);
    }
    if (!rows.length) note('正在读取…'); box.scrollTop = scroll;
  }
  async function load() {
    if (!started || busy || !available()) return;
    const wait = 1100 - (Date.now() - completedAt);
    if (wait > 0) { clearTimeout(deferred); deferred = setTimeout(load, wait); return; }
    busy = true; at = Date.now(); const run = epoch, selected = sort; let retry = false;
    try {
      const result = await request('processes', '/', {target:selected});
      if (run !== epoch || !started) return;
      records = (Array.isArray(result.processes) ? result.processes : []).filter(p=>typeof p.name==='string'&&Number.isFinite(p.memory)).slice(0,100);
      render(); if (!records.length) note('暂无进程数据');
    } catch (e) {
      if (run === epoch) {
        if (e.code === 'rate' || e.code === 'busy') { retry = true; if (!records.length) note('正在读取…'); }
        else { records = []; note(e.message || '进程暂不可用'); }
      }
    } finally {
      busy = false; completedAt = Date.now();
      if (run === epoch && started && (selected !== sort || retry)) { clearTimeout(deferred); deferred = setTimeout(load, 1100); }
    }
  }
  render();
  return {
    start() { if (!started) { started = true; load(); timer = setInterval(load, 5000); } },
    refresh() { if (Date.now() - at >= 5000) load(); },
    stop() { started = false; epoch++; clearInterval(timer); clearTimeout(deferred); records = []; note('连接后显示进程'); }
  };
}
export function createTerminalMonitor({root, getRecord, connected, request, onVolumes}) {
  const holder = root.querySelector('#terminal-monitor'), ip = node('div', 'terminal-node-ip');
  const uptime = node('div', 'terminal-system-line'), load = node('div', 'terminal-system-line', '负载 —');
  holder.append(ip, uptime, load);
  const bars = {};
  for (const [key, label] of [['cpu', 'CPU'], ['memory', '内存'], ['swap', '交换']]) {
    const row = node('div', 'terminal-resource'), heading = node('div'), name = node('span', '', label), text = node('span');
    const track = node('div', 'terminal-resource-track'), fill = node('i');
    track.append(fill); heading.append(name, text); row.append(heading, track); holder.append(row); bars[key] = {row, fill, text};
  }
  const processes = createProcessTable(holder, request, visible);
  const net = chart('网络', ['#c49b64', '#66a699']), latency = chart('主控通讯延时', ['#8fb8ca']);
  const selector = node('select', 'terminal-network-select'); selector.setAttribute('aria-label', '终端监测网卡');
  net.head.append(selector);
  const totals = node('div', 'terminal-network-totals');
  holder.append(net.box, totals, latency.box);
  const history = {rx: [], tx: [], latency: [], networkTimes: [], latencyTimes: []};
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  let timer, frame = 0, systemBusy = false, systemAt = 0, previousNames = '', started = false, epoch = 0;
  selector.onchange = () => { history.rx = []; history.tx = []; history.networkTimes = []; update(false); };
  const add = (key, v) => { history[key].push(v); if (history[key].length > 60) history[key].shift(); };
  const ms = n => Number.isFinite(n) ? n.toFixed(1) + ' ms' : '—';
  function visible() { return started && root.isConnected && !document.hidden && connected(); }
  function paint(now) {
    frame = 0; if (!visible()) return;
    net.render(now, !reduced.matches); latency.render(now, !reduced.matches);
    if (!reduced.matches) frame = requestAnimationFrame(paint);
  }
  function animate() { if (!frame && visible()) frame = requestAnimationFrame(paint); processes.refresh(); }
  function bar(key, used, total, caption) {
    const b = bars[key], valid = Number.isFinite(used) && Number.isFinite(total) && total > 0;
    b.fill.style.width = (valid ? Math.min(100, Math.max(0, used / total * 100)) : 0) + '%';
    b.text.textContent = caption || (valid ? Math.round(used / total * 100) + '% · ' + compactBytes(used) + ' / ' + compactBytes(total) : '—');
  }
  async function update(sample = true) {
    if (!started || !root.isConnected || document.hidden) return;
    const record = getRecord(); if (!record) return; const n = record.public, ready = connected() && n.online;
    ip.textContent = record.currentIP || record.ip; ip.title = record.username + '@' + record.ip + ':' + record.port;
    uptime.textContent = '运行 ' + (n.uptime || '—');
    bar('cpu', n.cpu, 100, Number.isFinite(n.cpu) ? n.cpu.toFixed(1) + '%' : '—');
    bar('memory', Number.isFinite(n.memory?.used) ? n.memory.used * 1073741824 : null, n.memory?.total * 1073741824);
    bars.swap.row.hidden = !(n.swap?.total > 0);
    if (n.swap?.total > 0) bar('swap', n.swap.used * 1073741824, n.swap.total * 1073741824);
    const interfaces = n.networkAvailable && Array.isArray(n.network) ? n.network : [];
    const names = interfaces.map(i => i.name).join('\0');
    if (names !== previousNames) {
      const selected = selector.value; selector.replaceChildren();
      for (const nic of interfaces) selector.append(new Option(nic.name, nic.name));
      selector.value = interfaces.some(i => i.name === selected) ? selected : (interfaces.find(i => i.default) || interfaces[0])?.name || '';
      previousNames = names;
      if (selector.value !== selected) { history.rx = []; history.tx = []; history.networkTimes = []; }
    }
    selector.hidden = !interfaces.length;
    const nic = interfaces.find(i => i.name === selector.value), rx = ready && nic ? nic.rx_rate : null, tx = ready && nic ? nic.tx_rate : null, rtt = ready ? n.latencyMs : null;
    const at = performance.now();
    while (history.networkTimes.length && at - history.networkTimes[0] > 120000) {
      history.networkTimes.shift(); history.rx.shift(); history.tx.shift();
    }
    while (history.latencyTimes.length && at - history.latencyTimes[0] > 120000) {
      history.latencyTimes.shift(); history.latency.shift();
    }
    if (sample || !history.latency.length) { add('latency', rtt); add('latencyTimes', at); }
    if (sample || !history.rx.length) { add('rx', rx); add('tx', tx); add('networkTimes', at); }
    net.draw([history.tx, history.rx], '↑ ' + compactBytes(tx) + '/s  ↓ ' + compactBytes(rx) + '/s', compactBytes, history.networkTimes, at);
    totals.textContent = '累计 ↑ ' + compactBytes(nic?.tx_bytes) + '  ↓ ' + compactBytes(nic?.rx_bytes);
    latency.draw([history.latency], ms(rtt), ms, history.latencyTimes, at);
    animate();
    if (ready && !systemBusy && Date.now() - systemAt > 15000) {
      systemBusy = true; systemAt = Date.now(); const run = epoch;
      try {
        const info = await request('system', '/');
        if (run !== epoch || !started) return;
        load.textContent = '负载 ' + (info.load?.length === 3 ? info.load.map(n => Number.isFinite(n) ? n.toFixed(2) : '—').join('  ') : '—');
        load.title = '1 / 5 / 15 分钟平均负载'; onVolumes?.(info.filesystems);
      } catch { if (run === epoch) load.textContent = '负载 —'; }
      finally { systemBusy = false; }
    }
  }
  const observer = new ResizeObserver(animate); observer.observe(holder);
  const resume = () => { if (!document.hidden) update(false); };
  document.addEventListener('visibilitychange', resume); reduced.addEventListener('change', animate);
  function stop() { clearInterval(timer); cancelAnimationFrame(frame); frame = 0; started = false; epoch++; systemAt = 0; processes.stop(); }
  return {
    start() { if (!started) { started = true; update(); processes.start(); timer = setInterval(update, 2000); } },
    refresh() { update(false); },
    stop,
    destroy() { stop(); observer.disconnect(); document.removeEventListener('visibilitychange', resume); reduced.removeEventListener('change', animate); }
  };
}
export function createRouteView({root, api, getRecord, connected, getTransfer}) {
  const view = root.querySelector('#terminal-route'), summary = root.querySelector('#terminal-route-summary');
  const tbody = root.querySelector('#terminal-route-rows'), status = root.querySelector('#terminal-route-status');
  let timer, busy = false, samples = 0, failures = 0, identity = '', stopped = true;
  async function poll() {
    if (stopped || busy || !root.isConnected || view.hidden || document.hidden || !connected() || !getTransfer()) return;
    const record = getRecord(); if (!record) return; busy = true;
    try {
      const data = await api('/api/admin/terminal-route', 'POST', {id: record.public.id, session: getTransfer()});
      if (stopped) return;
      if (identity !== getTransfer()) { identity = getTransfer(); samples = 0; failures = 0; poll.at = 0; }
      if (data.checkedAt && data.checkedAt !== poll.at) { poll.at = data.checkedAt; samples++; if (!data.reachable) failures++; }
      summary.textContent = data.checkedAt ? (data.reachable ? '● 连通' : '● 未连通') + '  ' + (Number.isFinite(data.latencyMs) ? data.latencyMs.toFixed(1) + ' ms' : '—') + '  · 检测 ' + samples + ' 次 · 失败 ' + failures + ' 次' : '正在检测连接…';
      summary.classList.toggle('unreachable', !!data.checkedAt && !data.reachable);
      tbody.replaceChildren();
      for (const hop of data.hops || []) {
        const row = node('tr'), owner = node('td', 'route-operator');
        const label = hop.operator || (hop.address ? (data.ownerLoading ? '识别中…' : '未知') : '—');
        const name = node('span', '', label); name.title = label; owner.append(name);
        if (hop.asns?.length) owner.append(node('small', '', hop.asns.map(n=>'AS'+n).join(' / ')));
        row.append(node('td', 'route-ttl', String(hop.ttl)), node('td', 'route-address', hop.address || '*'), owner, node('td', 'route-latency', Number.isFinite(hop.ms) ? hop.ms.toFixed(2) + ' ms' : '—'), node('td', '', hop.address ? (hop.reached ? '目标' : '已响应') : '未响应')); tbody.append(row);
      }
      const age = data.routeAt ? new Date(data.routeAt * 1000).toLocaleTimeString('zh-CN', {hour12: false}) : '';
      status.textContent = data.message || (data.tracing ? '正在追踪主控到被控的 UDP 路由…' : (data.routeAt ? '路由更新于 ' + age + '；* 表示该跳未回应，不代表终端连接中断。' : '等待路由检测…'));
      status.title = '连接性使用主控到服务器 SSH 端口的 TCP 探测；路由使用 UDP 探测。';
    } catch (e) { if (!stopped) status.textContent = e.message; }
    finally { busy = false; }
  }
  root.querySelector('#terminal-route-refresh').onclick = poll;
  return {
    start() { stopped = false; clearInterval(timer); poll(); timer = setInterval(poll, 5000); },
    refresh: poll,
    stop() { stopped = true; clearInterval(timer); },
    destroy() { stopped = true; clearInterval(timer); }
  };
}
