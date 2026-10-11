"""Prepare independent programs, models, profiles, writable assets and full database copies."""
import argparse,json,shutil
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--production',required=True,type=Path);p.add_argument('--snapshot',required=True,type=Path);p.add_argument('--workspace',required=True,type=Path);p.add_argument('--source',required=True,type=Path);a=p.parse_args()
production,snapshot,workspace,source=[v.resolve() for v in [a.production,a.snapshot,a.workspace,a.source]]
if workspace==production or workspace.is_relative_to(production):raise SystemExit('Workspace must be independent')
candidate=workspace/'candidate-1.1.0-rc.1';candidate.mkdir(exist_ok=True)
for relative in ['runtime','wd-swinv2-tagger-v3','model']:
    dst=candidate/relative
    if not dst.exists():shutil.copytree(production/relative,dst)
    print('Resources copied:',relative,flush=True)
shutil.copytree(source/'src-tauri/scripts',candidate/'scripts',dirs_exist_ok=True)
shutil.copy2(snapshot/'manifest.json',candidate/'baseline-resource-manifest.json')
(candidate/'illutag.portable.json').write_text('{}\n',encoding='utf-8')
for name in ['baseline','candidate']:
    root=workspace/'validation'/name;root.mkdir(parents=True,exist_ok=True)
    for sub in ['data','profile','assets','logs','pycache']:(root/sub).mkdir(exist_ok=True)
    db=root/'data/illutag.sqlite'
    if not db.exists():shutil.copy2(snapshot/'illutag.sqlite',db)
    print('Full independent database copied:',name,flush=True)
baseline=workspace/'baseline-program';baseline.mkdir(exist_ok=True)
shutil.copy2(snapshot/'illutag.exe',baseline/'illutag.exe')
shutil.copytree(snapshot/'scripts',baseline/'scripts',dirs_exist_ok=True)
for relative in ['runtime','wd-swinv2-tagger-v3','model']:
    dst=baseline/relative
    if not dst.exists():shutil.copytree(candidate/relative,dst,copy_function=shutil.copy2)
(baseline/'illutag.portable.json').write_text(json.dumps({'dataDir':str(workspace/'validation/baseline/data')}),encoding='utf-8')
print('Isolation prepared',flush=True)
