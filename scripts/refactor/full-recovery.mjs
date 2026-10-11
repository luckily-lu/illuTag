import {launch} from './native-session.mjs'
import fs from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
const [executable,workspace,existingPackage]=process.argv.slice(2);const origin=path.join(workspace,'validation/candidate'),target=path.join(workspace,'validation/full-restored'),destination=path.join(workspace,'validation/full-backups');fs.mkdirSync(destination,{recursive:true})
let session=await launch(executable,origin,9783),packageDir,manifest
try{const b=await session.invoke('get_bootstrap');assert.equal(b.page.totalImageCount,809214);
if(existingPackage){packageDir=existingPackage;console.log('Verifying existing complete snapshot',new Date().toISOString())}
else{console.log('Creating complete 809,230-image restore package',new Date().toISOString());packageDir=await session.invoke('create_restore_package',{destination,frontendSettings:await session.page.evaluate(()=>({localStorage:Object.fromEntries(Object.keys(localStorage).filter(k=>k.startsWith('illutag.')).map(k=>[k,localStorage.getItem(k)]))}))})}
manifest=await session.invoke('inspect_restore_package',{packageDir});assert.equal(manifest.counts.image_clip_embeddings,809229);assert.equal(manifest.counts.image_auto_tags,41254917);console.log('Full package verified',new Date().toISOString())}finally{await session.close()}
session=await launch(executable,target,9783)
try{await session.invoke('restore_package',{packageDir,pathMappings:[]});console.log('Restored independent full-library copy',new Date().toISOString())}finally{await session.close()}
session=await launch(executable,target,9783)
try{const b=await session.invoke('get_bootstrap');assert.equal(b.page.totalImageCount,809214);assert.equal(b.page.images.length,5000);assert.equal((await session.invoke('list_jobs')).filter(j=>j.state==='running').length,0);const last=await session.invoke('query_gallery',{query:{scope:'all',folderId:null,unclassifiedOnlyParentFolderId:null,tags:[],englishQuery:'',fileName:'',naturalLanguage:'',offset:805000,limit:5000,randomSeed:null}});assert.equal(last.images.length,4214);fs.writeFileSync(path.join(workspace,'validation/full-recovery.json'),JSON.stringify({passed:true,packageDir,restoredRoot:target,counts:manifest.counts,lastPageCount:last.images.length,schemaVersion:manifest.schemaVersion,externalOriginalsIncluded:false},null,2));console.log('Full recovery passed including final page')}finally{await session.close()}
