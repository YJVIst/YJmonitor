import test from 'node:test';
import assert from 'node:assert/strict';
import {matchesServer,duration,currentAddress,reportedAddressRows} from '../src/server-management.mjs';
const at=Date.parse('2026-10-08T00:00:00Z'),defaults={status:'all',query:'',group:null,provider:null,expiry:'all'};
const server={public:{name:'London 节点',online:true,group:'生产'},providerName:'Example Cloud',ip:'2001:db8::9',expiresAt:new Date(at+86400000).toISOString()};
test('admin filters combine status search group provider and expiry',()=>{
 assert(matchesServer(server,{...defaults,query:'lOnDoN',group:'生产',provider:'Example Cloud',expiry:'soon'},at));
 assert(matchesServer(server,{...defaults,query:'2001:db8::9'},at));
 assert(!matchesServer(server,{...defaults,status:'offline'},at));
 assert(!matchesServer(server,{...defaults,provider:'another'},at));
 assert(!matchesServer(server,{...defaults,query:'192.0.2.1'},at));
});
test('expiry boundaries do not depend on renewal notifications',()=>{
 const node={...server,notifyRenewal:false};
 assert(matchesServer({...node,expiresAt:new Date(at).toISOString()},{...defaults,expiry:'expired'},at));
 assert(matchesServer({...node,expiresAt:new Date(at+3*86400000).toISOString()},{...defaults,expiry:'soon'},at));
 assert(!matchesServer({...node,expiresAt:new Date(at+3*86400000+1).toISOString()},{...defaults,expiry:'soon'},at));
 assert(matchesServer({...node,expiresAt:''},{...defaults,expiry:'unset'},at));
 assert(!matchesServer({...node,expiresAt:'invalid'},{...defaults,expiry:'valid'},at));
});
test('pending nodes are offline and empty fields can be selected',()=>{
 assert(matchesServer({...server,public:{name:'待接入',online:false,pending:true},providerName:''},{...defaults,status:'offline',group:'',provider:''},at));
});
test('observed durations clamp clock skew and preserve units',()=>{
 assert.equal(duration(-50),'0 秒');assert.equal(duration(61),'1 分钟');
 assert.equal(duration(3600),'1 小时 0 分钟');assert.equal(duration(90061),'1 天 1 小时');
});

test('literal wildcard group and supplier remain selectable',()=>{
 const node={...server,providerName:'*',public:{...server.public,group:'*'}};
 assert(matchesServer(node,{...defaults,group:'*',provider:'*'},at));
 assert(!matchesServer(server,{...defaults,group:'*',provider:'*'},at));
});

import {pageWindow} from '../src/pagination.mjs';
test('1000 node pages stay bounded through filtering and deletion',()=>{
 assert.deepEqual(pageWindow(1000,20,50),{page:20,pages:20,start:950,end:1000});
 assert.deepEqual(pageWindow(1,20,50),{page:1,pages:1,start:0,end:1});
 assert.deepEqual(pageWindow(0,20,50),{page:1,pages:1,start:0,end:0});
 assert.equal(pageWindow(1000,99,60).end,1000);
});

test('reported egress addresses survive peer and DNS precedence and can be searched',()=>{
 const r={...server,detectedIPs:{ipv4:'192.0.2.8',ipv6:'2001:db8::8'},agentPeerIP:'192.0.2.7',resolvedIPs:['192.0.2.9']};
 assert.equal(currentAddress(r),'IPv4 192.0.2.8\nIPv6 2001:db8::8');
 for(const query of Object.values(r.detectedIPs))assert(matchesServer(r,{...defaults,query}));
 assert.deepEqual(reportedAddressRows({...r,detectedIPs:{ipv6:'2001:db8::8'}}),[['出口 IPv6','2001:db8::8']]);
 assert.equal(currentAddress({...r,detectedIPs:{ipv4:'192.0.2.8'}}),'IPv4 192.0.2.8');
 assert.equal(currentAddress({...r,detectedIPs:undefined}),'192.0.2.9');
 assert.deepEqual(reportedAddressRows({detectedIP:'192.0.2.1'}),[['上报 IP','192.0.2.1']]);
});
