import {launch,sleep,isolatedPath} from './native-session.mjs'
import {execFile} from 'node:child_process'
import {promisify} from 'node:util'
import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
const exec=promisify(execFile),[baselineArgument,candidateArgument,workspaceArgument,diagnoseArgument]=process.argv.slice(2)
if(diagnoseArgument&&diagnoseArgument!=='--diagnose-once')throw Error('Unknown benchmark option')
const baseline=isolatedPath(baselineArgument,'baseline program'),candidate=isolatedPath(candidateArgument,'candidate program'),workspace=isolatedPath(workspaceArgument,'workspace')
const output=path.join(workspace,'validation/performance.json'),runs=[],warmups=[],startedAt=new Date().toISOString()
const requestedWindowState={x:26,y:26,width:1312,height:838,isMaximized:false,isFullscreen:false}
let referenceViewport
const defaultQuery={scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null,tags:[],englishQuery:'',fileName:'',naturalLanguage:'',offset:0,limit:5000,randomSeed:null}
async function query(session,mode,kind){
 try{
  // Keep the application's IPC data in WebView2. Returning entire legacy ID
  // arrays through CDP adds unrelated transfer/serialization cost and can exceed
  // the debugging socket's payload bound. Only two scalar values leave the page.
  return await session.page.evaluate(async({mode,kind,defaultQuery})=>{
   const invoke=(command,args)=>window.__TAURI_INTERNALS__.invoke(command,args),begin=performance.now();let result
   if(mode==='candidate')result=await invoke('query_gallery',{query:{...defaultQuery,...(kind==='tags'?{tags:['1girl']}:kind==='file'?{fileName:'1'}:{naturalLanguage:'蓝色天空中的女孩'})}})
   else{
    const ids=kind==='semantic'
     ?await invoke('search_gallery_image_ids_by_natural_language_command',{query:'蓝色天空中的女孩',candidateImageIds:null})
     :await invoke('search_gallery_image_ids_command',{filters:{chineseTagEns:kind==='tags'?['1girl']:[],englishQuery:'',fileNameQuery:kind==='file'?'1':'',confidenceMin:0,confidenceMax:1,scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null}})
    result=await invoke('list_gallery_images_by_ids_page_command',{imageIds:ids,offset:0,limit:5000})
   }
   return{durationMs:performance.now()-begin,imageCount:result.images.length}
  },{mode,kind,defaultQuery})
 }catch(error){error.sessionDiagnostics=session.getDiagnostics();throw error}
}
async function memory(session){
 assert.equal(session.child.exitCode,null,'Cannot sample an exited validation process');assert.equal(session.child.signalCode,null);assert.ok(Number.isInteger(session.child.pid)&&session.child.pid>0)
 // Traverse only the launched process tree, including its WebView2 and Python
 // descendants. No names or global WebView2 process lists are used.
 const script=`$taskRootPid=${session.child.pid};$taskProcesses=@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId);$taskOwned=[System.Collections.Generic.HashSet[int]]::new();[void]$taskOwned.Add($taskRootPid);do{$taskAdded=$false;foreach($taskProcess in $taskProcesses){if($taskOwned.Contains([int]$taskProcess.ParentProcessId) -and $taskOwned.Add([int]$taskProcess.ProcessId)){$taskAdded=$true}}}while($taskAdded);$taskBytes=[long]0;$taskObserved=@();foreach($taskPid in $taskOwned){$taskHandle=Get-Process -Id $taskPid -ErrorAction SilentlyContinue;if($taskHandle){$taskBytes+=$taskHandle.WorkingSet64;$taskObserved+=$taskPid}};@{workingSetBytes=$taskBytes;pids=@($taskObserved|Sort-Object)}|ConvertTo-Json -Compress`
 const {stdout}=await exec('powershell',['-NoProfile','-Command',script],{windowsHide:true,timeout:15000});const sample=JSON.parse(stdout.trim());assert.ok(sample.pids.includes(session.child.pid));assert.ok(Number.isFinite(sample.workingSetBytes)&&sample.workingSetBytes>0);return sample
}
function percentile(values,p){assert.ok(values.length);const sorted=values.slice().sort((a,b)=>a-b);return sorted[Math.min(sorted.length-1,Math.ceil(sorted.length*p)-1)]}
function prepareWindow(root){const data=isolatedPath(path.join(root,'data'),'benchmark data');fs.mkdirSync(data,{recursive:true});const state=path.join(data,'window-state.json'),preserved=path.join(data,'performance-original-window-state.json');if(fs.existsSync(state)&&!fs.existsSync(preserved))fs.copyFileSync(state,preserved);fs.writeFileSync(state,JSON.stringify(requestedWindowState,null,2))}
function save(extra={}){fs.mkdirSync(path.dirname(output),{recursive:true});fs.writeFileSync(output,JSON.stringify({passed:false,startedAt,programs:{baseline,candidate},runs,warmups,method:{warmRuns:5,observationSeconds:90,requestedWindowState,viewport:referenceViewport,memory:'working sets of launched main process and its descendant WebView2/model processes only',startup:'launch to first gallery item painted',queryTiming:'WebView2 performance.now around complete Tauri IPC sequence; legacy search IDs + page request or candidate query_gallery; CDP returns only durationMs/imageCount',debuggingNetworkPostDataCaptureLimitBytes:1,debuggingCapture:'CDP Network.enable limits requestWillBeSent POST-body capture; actual application HTTP/Tauri request bodies are unchanged',querySamplesPerKind:25,queries:{tags:'1girl',file:'1',semantic:'蓝色天空中的女孩'}},...extra},null,2))}
if(diagnoseArgument){
 const root=isolatedPath(path.join(workspace,'validation/baseline'),'baseline data root');prepareWindow(root)
 const session=await launch(baseline,root,9784),reportPath=path.join(workspace,'validation/query-instrument-qa.json');let report
 try{
  await session.page.waitForSelector('[data-gallery-image-id]',{timeout:120000})
  const result=await query(session,'baseline','file');assert.ok(result.imageCount>0);assert.ok(Number.isFinite(result.durationMs)&&result.durationMs>=0)
  const beforeClose=session.getDiagnostics();assert.equal(beforeClose.exitCode,null);assert.equal(beforeClose.signalCode,null);assert.equal(beforeClose.connected,true)
  report={passed:true,instrumentCheckOnly:true,startedAt,query:'fileName=1',result,returnedBytes:Buffer.byteLength(JSON.stringify(result)),beforeClose,logPath:session.logPath}
 }catch(error){report={passed:false,instrumentCheckOnly:true,startedAt,error:error.stack??String(error),beforeClose:error.sessionDiagnostics??session.getDiagnostics(),logPath:session.logPath};throw error}
 finally{
  try{const begin=performance.now();await session.close();report.closeMs=performance.now()-begin;report.afterClose=session.getDiagnostics();if(report.afterClose.exitCode!==0||report.afterClose.connected)report.passed=false}
  finally{fs.writeFileSync(reportPath,JSON.stringify(report,null,2));console.log(JSON.stringify(report));if(!report.passed)process.exitCode=2}
 }
}else{
try{
for(const [mode,executable] of [['baseline',baseline],['candidate',candidate]]){
 const root=isolatedPath(path.join(workspace,'validation',mode),`${mode} data root`);assert.ok(fs.statSync(path.join(root,'data/illutag.sqlite')).size>29000000000,'Benchmark requires an independent full-library copy')
 for(let run=-1;run<5;run++){
  prepareWindow(root)
  const session=await launch(executable,root,9784);const timings={tags:[],file:[],semantic:[]},pids=new Set();let peak=0,startup=0,viewport,observationMs
  try{await session.page.waitForSelector('[data-gallery-image-id]',{timeout:120000});await session.page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));startup=performance.now()-session.started;
   viewport=await session.page.evaluate(()=>({width:innerWidth,height:innerHeight,scale:devicePixelRatio}));if(!referenceViewport)referenceViewport=viewport;assert.deepEqual(viewport,referenceViewport,'All runs must have the same actual CSS viewport and system scale');const start=performance.now();let tested=false
   async function sampleMemory(){const sample=await memory(session);peak=Math.max(peak,sample.workingSetBytes);for(const pid of sample.pids)pids.add(pid)}
   while(performance.now()-start<90000){await sampleMemory();if(!tested&&performance.now()-start>10000){for(const kind of ['tags','file','semantic']){const warm=await query(session,mode,kind);assert.ok(warm.imageCount>0);for(let n=0;n<5;n++){const result=await query(session,mode,kind);assert.ok(result.imageCount>0);assert.ok(Number.isFinite(result.durationMs)&&result.durationMs>=0);timings[kind].push(result.durationMs);await sampleMemory();}}tested=true;}await sleep(1000)}
   observationMs=performance.now()-start;assert.ok(tested);assert.deepEqual(await session.page.evaluate(()=>({width:innerWidth,height:innerHeight,scale:devicePixelRatio})),referenceViewport,'Window or scale changed during observation')
  }catch(error){error.sessionDiagnostics??=session.getDiagnostics();throw error}finally{await session.close()}
  const result={mode,run:run<0?'warmup':run+1,startupMs:startup,peakWorkingSetBytes:peak,queriesMs:timings,viewport,observationMs,sampledPids:[...pids],logPath:session.logPath};if(run<0)warmups.push(result);else runs.push(result);save()
  console.log(JSON.stringify({mode,run:result.run,startupMs:Math.round(startup),peakMB:Math.round(peak/1048576),viewport}))
 }
}
const metrics={};for(const mode of ['baseline','candidate']){const rows=runs.filter(r=>r.mode===mode);assert.equal(rows.length,5);metrics[mode]={startupP50:percentile(rows.map(r=>r.startupMs),.5),startupP90:percentile(rows.map(r=>r.startupMs),.9),peakMemory:Math.max(...rows.map(r=>r.peakWorkingSetBytes)),queryP95:Object.fromEntries(['tags','file','semantic'].map(k=>[k,percentile(rows.flatMap(r=>r.queriesMs[k]),.95)]))}}
const ratios={startupP50:metrics.candidate.startupP50/metrics.baseline.startupP50,startupP90:metrics.candidate.startupP90/metrics.baseline.startupP90,peakMemory:metrics.candidate.peakMemory/metrics.baseline.peakMemory,queryP95:Object.fromEntries(['tags','file','semantic'].map(k=>[k,metrics.candidate.queryP95[k]/metrics.baseline.queryP95[k]]))}
const passed=ratios.startupP50<=1.1&&ratios.startupP90<=1.15&&ratios.peakMemory<=1.1&&Object.values(ratios.queryP95).every(r=>r<=1.1)
save({passed,completedAt:new Date().toISOString(),metrics,ratios});console.log(JSON.stringify({passed,metrics,ratios}));if(!passed)process.exitCode=2
}catch(error){save({failedAt:new Date().toISOString(),error:error.stack??String(error),sessionDiagnostics:error.sessionDiagnostics});throw error}
}
