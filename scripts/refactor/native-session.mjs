import { chromium } from 'playwright'
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import {fileURLToPath} from 'node:url'
import path from 'node:path'
export const sleep=ms=>new Promise(r=>setTimeout(r,ms))
export async function launch(executable,root,port=9781){
  if(!path.isAbsolute(executable)||!path.isAbsolute(root)||!executable.includes('illuTag-refactor-work')||!root.includes('illuTag-refactor-work'))throw Error('Explicit isolated program and root required')
  for(const name of ['data','profile','assets','logs','pycache'])fs.mkdirSync(path.join(root,name),{recursive:true})
  const out=fs.openSync(path.join(root,'logs',`session-${Date.now()}.log`),'w')
  const started=performance.now()
  const child=spawn(executable,[],{cwd:path.dirname(executable),windowsHide:true,stdio:['ignore',out,out],env:{...process.env,
    ILLUTAG_DATA_ROOT:path.join(root,'data'),ILLUTAG_PROFILE_ROOT:path.join(root,'profile'),ILLUTAG_ASSET_ROOT:path.join(root,'assets'),ILLUTAG_LOG_ROOT:path.join(root,'logs'),
    ILLUTAG_RESOURCE_ROOT:path.dirname(executable),PYTHONPYCACHEPREFIX:path.join(root,'pycache'),
    WEBVIEW2_USER_DATA_FOLDER:path.join(root,'profile'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port} --remote-allow-origins=*`}})
  let browser
  for(let i=0;i<160;i++){if(child.exitCode!==null)throw Error('Application exited before CDP connected');try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);break}catch{await sleep(100)}}
  if(!browser){child.kill();throw Error('Independent WebView2 debugging endpoint unavailable')}
  const page=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('tauri'))??browser.contexts()[0].pages()[0]
  await page.waitForFunction(()=>window.__TAURI_INTERNALS__?.invoke,{timeout:30000})
  const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args})
  async function close(){try{await invoke('window_close_command')}catch{}for(let i=0;i<150&&child.exitCode===null;i++)await sleep(100);if(child.exitCode===null)child.kill();await browser.close().catch(()=>{});fs.closeSync(out)}
  return {child,browser,page,invoke,started,close}
}
if(path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
 const [executable,root]=process.argv.slice(2);const session=await launch(executable,root);try{await session.page.waitForSelector('[data-gallery-image-id]',{timeout:120000});console.log(JSON.stringify({url:session.page.url(),data:await session.invoke('data_directory_info_command'),firstScreenMs:performance.now()-session.started}));await session.page.screenshot({path:path.join(root,'logs','recovery-rehearsal.png')});await sleep(2000)}finally{await session.close()}
}
