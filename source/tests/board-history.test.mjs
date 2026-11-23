import test from 'node:test';
import assert from 'node:assert/strict';
import {historyPath,historyTransition} from '../src/board-history.mjs';

test('an early report continues from the current on-screen position',()=>{
 const history=Array.from({length:40},(_,i)=>i);
 const first=historyTransition(history,[...history.slice(1),41],true);
 const next=historyTransition(first.history,[...first.history.slice(1),42],true,first.offset/2);
 assert.equal(next.offset,first.offset*1.5);
 assert.deepEqual(next.points.slice(-3),[39,41,42]);
});

test('growing histories use the same pixel spacing as full histories',()=>{
 const transition=historyTransition([20],[20,30],true);
 assert(transition.offset>0);
 assert.equal(historyPath([20,30]).line.split(' L').at(-1),'110.00 28.20');
 assert(historyPath([20,30,40]).line.includes('107.23 28.20'));
});

test('returning to the tab or missing several samples snaps to current telemetry',()=>{
 const prior=[20,30];
 for(const [samples,continuous] of [[[20,30,40],false],[[30,40,50,60],true],[[4,5,6],true]]){
  const result=historyTransition(prior,samples,continuous,2);
  assert.equal(result.offset,0);
  assert.deepEqual(result.points,samples);
 }
});
