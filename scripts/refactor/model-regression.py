"""Compare deployment-baseline and refactored services with the same immutable models and inputs."""
import argparse,json,subprocess,hashlib,os
from pathlib import Path
import numpy as np
p=argparse.ArgumentParser();p.add_argument('--baseline',required=True,type=Path);p.add_argument('--candidate',required=True,type=Path);p.add_argument('--assets',required=True,type=Path);p.add_argument('--output',required=True,type=Path);a=p.parse_args()
images=sorted(a.assets.glob('*.png'))[:3]
if len(images)<3:raise SystemExit('Three fixed isolated PNG fixtures are required')
results={}
def run(program,kind,requests):
    python=program/'runtime/python/python.exe'
    if kind=='wd':args=[str(program/'scripts/wd_tagger_service.py'),'--model',str(program/'wd-swinv2-tagger-v3/model.onnx'),'--tags',str(program/'wd-swinv2-tagger-v3/selected_tags.csv')]
    else:args=[str(program/f'scripts/chinese_clip_{kind}_service.py'),'--model-dir',str(program/'model/chinese-clip-vit-base-patch16'),'--provider','cpu']
    payload=''.join(json.dumps(r,ensure_ascii=False)+'\n' for r in requests)
    completed=subprocess.run([str(python),'-X','utf8',*args],input=payload,text=True,encoding='utf-8',capture_output=True,timeout=180,creationflags=subprocess.CREATE_NO_WINDOW,env={**os.environ,'PYTHONPYCACHEPREFIX':str(a.output.parent/'pycache')})
    if completed.returncode:raise RuntimeError(completed.stderr)
    responses=[json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
    if len(responses)!=len(requests) or any('error' in r for r in responses):raise RuntimeError(str(responses))
    return responses
wd_requests=[{'image_id':str(i),'image_path':str(i),'general_threshold':.35,'character_threshold':.85} for i in images]
old,new=[run(root,'wd',wd_requests) for root in [a.baseline,a.candidate]]
for before,after in zip(old,new):
    for field in ['ratings','generalTags','characterTags','generalThreshold','characterThreshold']:assert before[field]==after[field],field
    assert after['generalThreshold']==.2 and after['characterThreshold']==.2
results['wd']={'samples':3,'tagsAndScoresEqual':True,'generalThreshold':.2,'characterThreshold':.2}
image_old,image_new=[run(root,'image',[{'image_path':str(i)} for i in images]) for root in [a.baseline,a.candidate]]
texts=['蓝色背景上的圆形','竖构图，柔和的色彩','抽象风景'];text_old,text_new=[run(root,'text',[{'text':t} for t in texts]) for root in [a.baseline,a.candidate]]
for kind,before,after in [('image',image_old,image_new),('text',text_old,text_new)]:
    errors=[float(np.max(np.abs(np.asarray(x['embedding'])-np.asarray(y['embedding'])))) for x,y in zip(before,after)]
    assert max(errors)==0,errors;results[kind]={'samples':3,'maxAbsoluteDifference':max(errors)}
before=np.asarray([r['embedding'] for r in text_old])@np.asarray([r['embedding'] for r in image_old]).T
after=np.asarray([r['embedding'] for r in text_new])@np.asarray([r['embedding'] for r in image_new]).T
assert np.array_equal(np.argsort(-before,axis=1),np.argsort(-after,axis=1));results['ranking']={'equal':True,'queries':texts}
a.output.parent.mkdir(parents=True,exist_ok=True);a.output.write_text(json.dumps({'passed':True,'results':results,'inputHashes':{i.name:hashlib.sha256(i.read_bytes()).hexdigest() for i in images}},ensure_ascii=False,indent=2),encoding='utf-8');print(json.dumps(results,ensure_ascii=False))
