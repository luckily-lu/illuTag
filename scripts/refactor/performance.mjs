import {launch,sleep} from './native-session.mjs'
import {execFile} from 'node:child_process'
import {promisify} from 'node:util'
import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
const exec=promisify(execFile),[baseline,candidate,workspace]=process.argv.slice(2)
if(!baseline||!candidate||!workspace.includes('illuTag-refactor-work'))throw Error('Explicit isolated programs and workspace required')
const output=path.join(workspace,'validation/performance.json'),runs=[]
const defaultQuery={scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null,tags:[],englishQuery:'',fileName:'',naturalLanguage:'',offset:0,limit:5000,randomSeed:null}
async function query(session,mode,kind){
 if(mode==='candidate')return session.invoke('query_gallery',{query:{...defaultQuery,...(kind==='tags'?{tags:['1girl']}:kind==='file'?{fileName:'1'}:{naturalLanguage:'蓝色天空中的女孩'})}})
 if(kind==='semantic'){const ids=await session.invoke('search_gallery_image_ids_by_natural_language_command',{query:'蓝色天空中的女孩',candidateImageIds:null});return session.invoke('list_gallery_images_by_ids_page_command',{imageIds:ids,offset:0,limit:5000})}
 const ids=await session.invoke('search_gallery_image_ids_command',{filters:{chineseTagEns:kind==='tags'?['1girl']:[],englishQuery:'',fileNameQuery:kind==='file'?'1':'',confidenceMin:0,confidenceMax:1,scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null}});return session.invoke('list_gallery_images_by_ids_page_command',{imageIds:ids,offset:0,limit:5000})
}
async function memory(session){const cdp=await session.browser.newBrowserCDPSession();const info=await cdp.send('SystemInfo.getProcessInfo');await cdp.detach();const ids=[session.child.pid,...info.processInfo.map(p=>p.id)].map(Number).filter(Number.isFinite);const script=`$taskIds=@(${ids.join(',')});$taskIds+=@(Get-CimInstance Win32_Process -Filter 'ParentProcessId=${session.child.pid}' | Select-Object -ExpandProperty ProcessId);$sum=0;foreach($taskPid in ($taskIds|Select-Object -Unique)){ $p=Get-Process -Id $taskPid -ErrorAction SilentlyContinue;if($p){$sum+=$p.WorkingSet64}};Write-Output $sum`;const {stdout}=await exec('powershell',['-NoProfile','-Command',script],{windowsHide:true});return Number(stdout.trim())}
function percentile(values,p){const sorted=values.slice().sort((a,b)=>a-b);return sorted[Math.min(sorted.length-1,Math.ceil(sorted.length*p)-1)]}
function save(){fs.writeFileSync(output,JSON.stringify({runs},null,2))}
for(const [mode,executable] of [['baseline',baseline],['candidate',candidate]]){
 for(let run=-1;run<5;run++){
  const session=await launch(executable,path.join(workspace,'validation',mode),9784);const timings={tags:[],file:[],semantic:[]};let peak=0,startup=0
  try{await session.page.waitForSelector('[data-gallery-image-id]',{timeout:120000});await session.page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));startup=performance.now()-session.started;const start=performance.now();let tested=false
   while(performance.now()-start<90000){peak=Math.max(peak,await memory(session));if(!tested&&performance.now()-start>10000){for(const kind of ['tags','file','semantic']){await query(session,mode,kind);for(let n=0;n<5;n++){const begin=performance.now();const result=await query(session,mode,kind);assert.ok(result.images.length>0);timings[kind].push(performance.now()-begin);peak=Math.max(peak,await memory(session));}}tested=true;}await sleep(1000)}
  }finally{await session.close()}
  if(run>=0){runs.push({mode,run:run+1,startupMs:startup,peakWorkingSetBytes:peak,queriesMs:timings});save()}
  console.log(JSON.stringify({mode,run:run<0?'warmup':run+1,startupMs:Math.round(startup),peakMB:Math.round(peak/1048576)}))
 }
}
const metrics={};for(const mode of ['baseline','candidate']){const rows=runs.filter(r=>r.mode===mode);metrics[mode]={startupP50:percentile(rows.map(r=>r.startupMs),.5),startupP90:percentile(rows.map(r=>r.startupMs),.9),peakMemory:Math.max(...rows.map(r=>r.peakWorkingSetBytes)),queryP95:Object.fromEntries(['tags','file','semantic'].map(k=>[k,percentile(rows.flatMap(r=>r.queriesMs[k]),.95)]))}}
const ratios={startupP50:metrics.candidate.startupP50/metrics.baseline.startupP50,startupP90:metrics.candidate.startupP90/metrics.baseline.startupP90,peakMemory:metrics.candidate.peakMemory/metrics.baseline.peakMemory,queryP95:Object.fromEntries(['tags','file','semantic'].map(k=>[k,metrics.candidate.queryP95[k]/metrics.baseline.queryP95[k]]))}
const passed=ratios.startupP50<=1.1&&ratios.startupP90<=1.15&&ratios.peakMemory<=1.1&&Object.values(ratios.queryP95).every(r=>r<=1.1)
fs.writeFileSync(output,JSON.stringify({passed,metrics,ratios,runs,method:{warmRuns:5,observationSeconds:90,viewport:'restored 1280×820 window',memory:'main + isolated WebView2 + direct model children working sets',startup:'launch to first gallery item painted',querySamplesPerKind:25}},null,2));console.log(JSON.stringify({passed,metrics,ratios}));if(!passed)process.exitCode=2
