// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.
import { afterEach, expect, it, vi } from 'vitest';
import { installIsolatedRustWorker, type RustWorkerScope } from './isolatedRustWorker';
class FakeWorker {
  static all: FakeWorker[] = [];
  onmessage?: (event: {data: unknown}) => void;
  onerror?: (event: {message: string}) => void;
  onmessageerror?: () => void;
  postMessage=vi.fn(); terminate=vi.fn();
  constructor(public url: string, public options: WorkerOptions){FakeWorker.all.push(this);}
  emit(data: unknown){this.onmessage?.({data});}
}
const setup=()=>{
 FakeWorker.all=[];vi.stubGlobal('Worker',FakeWorker);
 const scope={location:{href:'https://example.com/rust-worker.mjs'},postMessage:vi.fn(),onmessage:null as RustWorkerScope['onmessage']};
 installIsolatedRustWorker(scope);
 const send=(data:unknown)=>scope.onmessage!(new MessageEvent('message',{data}));
 const ready=(w:FakeWorker)=>w.emit({type:'worker_ready',targetBundleId:'rust'});
 send({type:'initialize',manifest:{bundleId:'rust'}});ready(FakeWorker.all[0]);
 return {scope,send,ready};
};
afterEach(()=>{vi.unstubAllGlobals();vi.useRealTimers();});
it('isolates every Rust job and preserves ordered writes, progress and identity',()=>{
 const {scope,send,ready}=setup(),jobs=[{name:'first'},{name:'second'}];
 send({type:'run_prepared_batch',targetBundleId:'rust',jobId:'batch',jobs});
 const first=FakeWorker.all[0];expect(first.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({jobs:[jobs[0]]}));
 const write={type:'file_system',operation:'write_file',messageId:'last-write',path:'output/first/target-run.json'};
 first.emit(write);expect(scope.postMessage).toHaveBeenLastCalledWith(write);expect(FakeWorker.all).toHaveLength(1);
 const ack={type:'file_system_response',messageId:'last-write'};send(ack);expect(first.postMessage).toHaveBeenLastCalledWith(ack);
 const terminal={type:'complete',jobId:'batch',targetBundleId:'rust'};
 first.emit(terminal);expect(first.terminate).toHaveBeenCalledOnce();expect(FakeWorker.all).toHaveLength(2);
 const second=FakeWorker.all[1];expect(second.options.name).toBe('vulcan-rust-execution');ready(second);
 expect(second.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({jobs:[jobs[1]],jobId:'batch'}));
 first.emit({type:'error',error:'late'});first.onerror?.({message:'late trap'});first.onmessageerror?.();expect(second.terminate).not.toHaveBeenCalled();
 second.emit(terminal);expect(second.terminate).toHaveBeenCalledOnce();expect(scope.postMessage).toHaveBeenLastCalledWith(terminal);
 expect(scope.postMessage.mock.calls.filter(([m])=>m.type==='progress').map(([m])=>m.progress)).toEqual([{completed:1,total:2},{completed:2,total:2}]);
});
it.each(['error','init_error'])('stops after child %s and never starts the next job',type=>{
 const {scope,send}=setup();send({type:'run_prepared_batch',targetBundleId:'rust',jobId:'batch',jobs:[{},{}]});
 FakeWorker.all[0].emit({type,error:'failed write or calculation'});
 expect(FakeWorker.all).toHaveLength(1);expect(FakeWorker.all[0].terminate).toHaveBeenCalledOnce();
 expect(scope.postMessage).toHaveBeenLastCalledWith({type:'error',jobId:'batch',error:'failed write or calculation'});
});
it('reports an execution trap and rejects wrong target completion',()=>{
 const {scope,send}=setup();send({type:'run_prepared_batch',targetBundleId:'rust',jobId:'batch',jobs:[{}]});
 FakeWorker.all[0].onerror?.({message:'unreachable'});expect(scope.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({error:'unreachable'}));
 send({type:'run_prepared_batch',targetBundleId:'rust',jobId:'next',jobs:[{}]});
 FakeWorker.all[1].emit({type:'complete',jobId:'wrong',targetBundleId:'rust'});
 expect(scope.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({error:'Completed worker identity mismatch'}));
});

it('bounds each replacement startup without bounding annual calculation',()=>{
 vi.useFakeTimers();const {scope,send}=setup();send({type:'run_prepared_batch',targetBundleId:'rust',jobId:'batch',jobs:[{},{}]});
 vi.advanceTimersByTime(200_000);expect(FakeWorker.all[0].terminate).not.toHaveBeenCalled();
 FakeWorker.all[0].emit({type:'complete',jobId:'batch',targetBundleId:'rust'});vi.advanceTimersByTime(120_000);
 expect(FakeWorker.all[1].terminate).toHaveBeenCalledOnce();expect(scope.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({error:'Rust execution worker startup exceeded 120 seconds'}));
});

it('routes rejected batch errors back to the submitting caller',()=>{
 const {scope,send}=setup();send({type:'run_prepared_batch',targetBundleId:'wrong',jobId:'rejected',jobs:[{}]});
 expect(scope.postMessage).toHaveBeenLastCalledWith(expect.objectContaining({type:'error',jobId:'rejected'}));
});
it.each([['./rustFhsWorker','Rust target is not initialized'],['./pythonFhsWorker','Python worker is not initialized']])('%s reports a failure in its own words, without a stringified "Error: " prefix',async(module,message)=>{
 vi.resetModules();
 const scope={name:'vulcan-rust-execution',postMessage:vi.fn(),onmessage:undefined as undefined|((event:{data:unknown})=>Promise<void>)};
 vi.stubGlobal('self',scope);
 await import(/* @vite-ignore */ module);
 await scope.onmessage!({data:{type:'run_prepared_batch',jobId:'batch',targetBundleId:'rust',jobs:[]}});
 expect(scope.postMessage).toHaveBeenCalledWith({type:'error',jobId:'batch',error:message});
});
