import {connectDebugging,sleep} from './native-session.mjs'
import {spawn,execFile} from 'node:child_process'
import {promisify} from 'node:util'
import fs from 'node:fs'
import path from 'node:path'
import {createHash} from 'node:crypto'
import assert from 'node:assert/strict'

const exec=promisify(execFile)
const [executable,database,workspace,resources,mode='benchmark']=process.argv.slice(2)
for(const value of [executable,database,workspace,resources])assert.ok(value&&path.isAbsolute(value),'Explicit absolute paths required')
assert.ok(path.resolve(workspace).toLowerCase().startsWith('d:\\ai\\illutag-storage-work\\'),'Storage validation workspace required')
assert.ok(['benchmark','smoke'].includes(mode))
for(const suffix of ['-wal','-journal'])assert.ok(!fs.existsSync(database+suffix)||fs.statSync(database+suffix).size===0,'Source must be settled')
for(const name of ['data','profile','assets','logs','pycache','cuda'])fs.mkdirSync(path.join(workspace,name),{recursive:true})
const linked=path.join(workspace,'data','illutag.sqlite')
assert.ok(!fs.existsSync(linked),'Validation database link already exists')
fs.linkSync(database,linked) // Read-only NTFS reference: no database copy.
fs.writeFileSync(path.join(workspace,'data','window-state.json'),JSON.stringify({x:26,y:26,width:1312,height:838,isMaximized:false,isFullscreen:false}))
const query={scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null,tags:[],englishQuery:'',fileName:'',naturalLanguage:'',offset:0,limit:5000,randomSeed:null}
const runs=[]
const percentile=(xs,p)=>xs.slice().sort((a,b)=>a-b)[Math.min(xs.length-1,Math.ceil(xs.length*p)-1)]
async function memory(pid){
 const code=`$taskProcesses=@(Get-CimInstance Win32_Process|Select-Object ProcessId,ParentProcessId);$taskOwned=[System.Collections.Generic.HashSet[int]]::new();[void]$taskOwned.Add(${pid});do{$taskAdded=$false;foreach($p in $taskProcesses){if($taskOwned.Contains([int]$p.ParentProcessId)-and $taskOwned.Add([int]$p.ProcessId)){$taskAdded=$true}}}while($taskAdded);$bytes=[long]0;foreach($p in $taskOwned){$h=Get-Process -Id $p -ErrorAction SilentlyContinue;if($h){$bytes+=$h.WorkingSet64}};$bytes`
 const {stdout}=await exec('powershell',['-NoProfile','-Command',code],{windowsHide:true,timeout:15000});return Number(stdout.trim())
}
async function run(number,warmup=false){
 fs.writeFileSync(path.join(workspace,'data','window-state.json'),JSON.stringify({x:26,y:26,width:1312,height:838,isMaximized:false,isFullscreen:false}))
 const started=performance.now(),port=9793
 const log=fs.openSync(path.join(workspace,'logs',`run-${number}.log`),'w')
 const env={...process.env,ILLUTAG_READ_ONLY:'1',ILLUTAG_DATA_ROOT:path.join(workspace,'data'),ILLUTAG_PROFILE_ROOT:path.join(workspace,'profile'),ILLUTAG_ASSET_ROOT:path.join(workspace,'assets'),ILLUTAG_LOG_ROOT:path.join(workspace,'logs'),ILLUTAG_RESOURCE_ROOT:resources,PYTHONPYCACHEPREFIX:path.join(workspace,'pycache'),CUDA_CACHE_PATH:path.join(workspace,'cuda'),WEBVIEW2_USER_DATA_FOLDER:path.join(workspace,'profile'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-allow-origins=*`,TEMP:path.join(workspace,'logs'),TMP:path.join(workspace,'logs')}
 const child=spawn(executable,[],{cwd:path.dirname(executable),windowsHide:true,stdio:['ignore',log,log],env})
 let browser,transport,page,error
 child.on('error',e=>error=e)
 const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args})
 try{
  for(let n=0;n<300;n++){
   if(error)throw error
   assert.equal(child.exitCode,null,'Validation program exited')
   try{({browser,transport}=await connectDebugging(port));break}catch{await sleep(100)}
  }
  assert.ok(browser)
  for(let n=0;n<300;n++){
   page=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('tauri.localhost'))
   if(page)break
   await sleep(100)
  }
  assert.ok(page)
  await page.waitForSelector('[data-gallery-image-id]',{timeout:120000})
  const firstScreenMs=performance.now()-started
  const viewport=await page.evaluate(()=>({width:innerWidth,height:innerHeight,dpr:devicePixelRatio}))
  assert.ok(viewport.width>=1000&&viewport.height>=600,'Benchmark window failed to restore its requested size')
  if(warmup)return
  const timings={tags:[],file:[],semantic:[]},digests={},counts={}
  const bootstrap=await invoke('get_bootstrap')
  const detail=await invoke('get_image_detail',{imageId:bootstrap.page.images[0].id})
  assert.ok(detail.image&&detail.autoTags)
  const detailDigest=createHash('sha256').update(JSON.stringify(detail)).digest('hex')
  let peak=await memory(child.pid),sampledAt=performance.now()
  for(const kind of Object.keys(timings)){
   const q={...query,...(kind==='tags'?{tags:['1girl']}:kind==='file'?{fileName:'1'}:{naturalLanguage:'蓝色天空中的女孩'})}
   const collect=()=>page.evaluate(async query=>{
    const begin=performance.now();let result
    try{result=await window.__TAURI_INTERNALS__.invoke('query_gallery',{query})}
    catch(error){throw new Error(JSON.stringify(error))}
    return{ms:performance.now()-begin,ids:result.images.map(i=>i.id),total:result.totalImageCount}
   },q)
   const cold=await collect();digests[kind]=createHash('sha256').update(JSON.stringify(cold.ids)).digest('hex');counts[kind]=cold.total
   for(let n=0;n<(mode==='smoke'?2:25);n++){
    const result=await collect()
    assert.equal(createHash('sha256').update(JSON.stringify(result.ids)).digest('hex'),digests[kind])
    timings[kind].push(result.ms)
    if(performance.now()-sampledAt>10000){peak=Math.max(peak,await memory(child.pid));sampledAt=performance.now()}
   }
  }
  while(mode==='benchmark'&&performance.now()-started<90000){
   await sleep(Math.min(10000,90000-(performance.now()-started)))
   peak=Math.max(peak,await memory(child.pid))
  }
  const row={number,firstScreenMs,peakBytes:peak,viewport,digests,counts,detailDigest,timings}
  runs.push(row)
  console.log(JSON.stringify({phase:'native_run',number,firstScreenMs,peakBytes:peak,digests,counts}))
 }finally{
  if(page&&child.exitCode===null)await invoke('window_close_command').catch(()=>{})
  for(let n=0;n<150&&child.exitCode===null;n++)await sleep(100)
  assert.notEqual(child.exitCode,null,'Normal validation shutdown failed')
  if(browser){const close=browser.close();transport?.close();await Promise.race([close,sleep(5000)])}
  fs.closeSync(log)
 }
}
await run(0,true)
for(let i=1;i<=(mode==='smoke'?1:5);i++)await run(i)
const summary={startupP50:percentile(runs.map(r=>r.firstScreenMs),.5),startupP90:percentile(runs.map(r=>r.firstScreenMs),.9),peakBytes:Math.max(...runs.map(r=>r.peakBytes)),queryP95:Object.fromEntries(['tags','file','semantic'].map(k=>[k,percentile(runs.flatMap(r=>r.timings[k]),.95)])),digests:runs[0].digests,counts:runs[0].counts,detailDigest:runs[0].detailDigest}
console.log(JSON.stringify({phase:'native_summary',summary,runs:runs.map(({timings,...r})=>r)}))
