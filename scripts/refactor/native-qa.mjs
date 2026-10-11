import { launch,sleep } from './native-session.mjs'
import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
import {execFileSync} from 'node:child_process'
const [executable,root]=process.argv.slice(2);if(!root.includes('validation'))throw Error('QA requires an explicit validation directory')
const assets=path.join(root,'assets');fs.mkdirSync(assets,{recursive:true})
const python=path.join(path.dirname(executable),'runtime/python/python.exe')
execFileSync(python,['-c',"from PIL import Image,ImageDraw;import pathlib,sys;p=pathlib.Path(sys.argv[1]);sizes=[(800,600),(600,1000),(250,3000)];\nfor i,(w,h) in enumerate(sizes):\n im=Image.new('RGB',(w,h),(100+i*40,160,180));d=ImageDraw.Draw(im);d.ellipse((w*.1,min(h,w)*.1,w*.8,min(h*.8,w*.8)),fill=(230,210,170));im.save(p/f'sample-{i}.png')",assets],{windowsHide:true})
const checks=[],session=await launch(executable,root,9782)
const {invoke,page}=session
async function waitJob(id){for(let i=0;i<1200;i++){const row=(await invoke('list_jobs')).find(r=>r.id===id);if(['completed','partial','cancelled'].includes(row.state))return row;await sleep(200)}throw Error('Job did not finish')}
try{
assert.equal(page.url(),'http://tauri.localhost/');const initial=await invoke('get_bootstrap');assert.equal(initial.page.totalImageCount,0);await invoke('add_gallery_folder_command',{folderPath:assets});let b=await invoke('get_bootstrap');assert.equal(b.folders[0].lastScannedAt,null);assert.equal(b.page.totalImageCount,0);assert.equal((await invoke('list_jobs')).length,0);checks.push('adding a source does not scan')
const pipeline=await invoke('start_job',{request:{kind:'pipeline',thumbnails:true,sourceFolderId:null}});let paused=false
for(let i=0;i<1200;i++){const j=(await invoke('list_jobs')).find(r=>r.id===pipeline.id);if(!paused&&j.state==='running'&&j.stage==='semantic'){await invoke('control_job',{id:j.id,action:'pause'});await sleep(400);assert.equal((await invoke('list_jobs')).find(r=>r.id===j.id).state,'paused');await invoke('control_job',{id:j.id,action:'resume'});paused=true;}if(['completed','partial'].includes(j.state))break;await sleep(200)}
const finished=await waitJob(pipeline.id);assert.equal(finished.state,'completed',JSON.stringify(finished.errors));assert.equal(finished.failures,0);checks.push('pipeline scan → thumbnails → semantic → tagging', 'pause and resume during processing')
b=await invoke('get_bootstrap');assert.equal(b.page.totalImageCount,3);const image=b.page.images[0];const detail=await invoke('get_image_detail',{imageId:image.id});assert.ok(detail.autoTags.generalTags.length>0);assert.ok(image.thumbnailPath);const nav=await invoke('get_navigation');assert.equal('page' in nav,false);assert.ok(nav.folders[0].lastScannedAt);checks.push('real ONNX tagger and thumbnails','navigation metadata excludes gallery snapshots')
const query={scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null,tags:[],englishQuery:'',fileName:'',naturalLanguage:'蓝色背景上的圆形',offset:0,limit:5000,randomSeed:null};const semantic=await invoke('query_gallery',{query});assert.equal(semantic.images.length,3);checks.push('natural language uses generated CLIP image embeddings')
const result=await invoke('mutate_images',{request:{action:'favorite',imageIds:[image.id,'missing'],folderId:null,favorite:true,customTags:[],supplementTags:[]}});assert.equal(result.successfulIds.length,1);assert.equal(result.failures.length,1);assert.equal(result.images.length,1);checks.push('bounded mutation and item failure')
await invoke('create_user_folder_command',{parentId:null,name:'测试分类'});b=await invoke('get_bootstrap');const folder=b.userFolders.find(f=>f.name==='测试分类');await invoke('mutate_images',{request:{action:'assign',imageIds:[image.id],folderId:folder.id,favorite:null,customTags:[],supplementTags:[]}});assert.ok((await invoke('get_image_detail',{imageId:image.id})).folderIds.includes(folder.id));checks.push('virtual folder membership')
await invoke('copy_image_to_system_clipboard_command',{imageId:image.id});checks.push('original image copied to Windows clipboard')
await invoke('window_toggle_always_on_top_command');await invoke('window_toggle_always_on_top_command');await invoke('window_toggle_maximize_command');await invoke('window_toggle_maximize_command');checks.push('real window controls')
await page.reload();await page.waitForSelector('[data-gallery-image-id]');await page.screenshot({path:path.join(root,'logs','native-workbench.png')});await page.getByRole('button',{name:'查看 '+image.fileName,exact:true}).click();await page.waitForSelector('.viewer');await page.getByRole('button',{name:'信息',exact:true}).click();await page.screenshot({path:path.join(root,'logs','native-viewer.png')});await page.keyboard.press('Escape');checks.push('viewer in WebView2')
const backupRoot=path.join(root,'backups');fs.mkdirSync(backupRoot,{recursive:true});const packageDir=await invoke('create_restore_package',{destination:backupRoot,frontendSettings:{localStorage:{'illutag.themeMode':'light'}}});const inspected=await invoke('inspect_restore_package',{packageDir});assert.equal(inspected.imageCount,3);assert.equal(inspected.counts.image_clip_embeddings,3);assert.ok(inspected.counts.image_auto_tags>0);checks.push('complete SQLite package integrity')
await invoke('restore_package',{packageDir,pathMappings:[]});let denied=false;try{await invoke('start_job',{request:{kind:'scan',thumbnails:false,sourceFolderId:null}})}catch{denied=true}assert.ok(denied);checks.push('restoration requires restart before writing')
fs.writeFileSync(path.join(root,'logs','native-qa.json'),JSON.stringify({passed:true,checks,pipeline:finished,packageDir,thresholds:{general:.2,character:.2}},null,2));console.log(JSON.stringify({passed:true,checks}))
}finally{await session.close()}

