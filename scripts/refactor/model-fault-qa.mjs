import {launch,sleep} from './native-session.mjs';
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
const [executable,workspace]=process.argv.slice(2);
if(!workspace?.includes('illuTag-refactor-work'))throw Error('Explicit isolated workspace required');
const program=path.join(workspace,'validation/fault-program'),root=path.join(workspace,'validation/model-fault-qa');
fs.mkdirSync(path.join(program,'runtime/python'),{recursive:true});
fs.mkdirSync(path.join(program,'scripts'),{recursive:true});
fs.mkdirSync(path.join(program,'wd-swinv2-tagger-v3'),{recursive:true});
fs.mkdirSync(path.join(root,'assets'),{recursive:true});
fs.copyFileSync(executable,path.join(program,'illutag.exe'));
fs.writeFileSync(path.join(program,'illutag.portable.json'),'{}');
const resources=path.dirname(executable);
for(const file of fs.readdirSync(path.join(resources,'runtime/python'),{withFileTypes:true}).filter(e=>e.isFile()))fs.copyFileSync(path.join(resources,'runtime/python',file.name),path.join(program,'runtime/python',file.name));
for(const file of ['selected_tags.csv','selected_tags_full_translation.csv'])fs.copyFileSync(path.join(resources,'wd-swinv2-tagger-v3',file),path.join(program,'wd-swinv2-tagger-v3',file));
fs.writeFileSync(path.join(program,'wd-swinv2-tagger-v3/model.onnx'),'fault fixture');
fs.writeFileSync(path.join(program,'scripts/wd_tagger_service.py'),`import sys,json,os,pathlib
r=json.loads(sys.stdin.readline())
with open(pathlib.Path(os.environ['ILLUTAG_LOG_ROOT'])/'requests.jsonl','a',encoding='utf8') as f:f.write(json.dumps(r)+'\\n')
sys.exit(2)
`);
for(const file of fs.readdirSync(path.join(workspace,'validation/native-qa-2/assets')))fs.copyFileSync(path.join(workspace,'validation/native-qa-2/assets',file),path.join(root,'assets',file));
const session=await launch(path.join(program,'illutag.exe'),root,9786);
async function wait(id){for(let n=0;n<150;n++){const j=(await session.invoke('list_jobs')).find(r=>r.id===id);if(['completed','partial','cancelled'].includes(j.state))return j;await sleep(200);}throw Error('Failed model job did not terminate in 30 seconds');}
try{
 await session.invoke('add_gallery_folder_command',{folderPath:path.join(root,'assets')});
 const scan=await session.invoke('start_job',{request:{kind:'scan',thumbnails:false,sourceFolderId:null}});assert.equal((await wait(scan.id)).state,'completed');
 const job=await session.invoke('start_job',{request:{kind:'tagging',thumbnails:false,sourceFolderId:null}}),result=await wait(job.id);
 assert.equal(result.state,'partial');assert.equal(result.failures,3);assert.equal(result.processed,3);
 const requests=fs.readFileSync(path.join(root,'logs/requests.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
 const attempts={};for(const r of requests)attempts[r.image_id]=(attempts[r.image_id]??0)+1;
 assert.equal(Object.keys(attempts).length,3);assert.deepEqual(Object.values(attempts),[2,2,2]);
 const outcome={passed:true,check:'exited model receives one retry per image, reaches partial terminal state, advances across prefetch queue',attempts,result};
 fs.writeFileSync(path.join(root,'logs/model-fault-qa.json'),JSON.stringify(outcome,null,2));console.log(JSON.stringify(outcome));
}finally{await session.close();}
