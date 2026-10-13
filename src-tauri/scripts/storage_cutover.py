"""Journalled same-volume schema upgrade. Backups are renames of the original DB.

prepare/install keep the app blocked until explicit acceptance. recover restores
the original DB/program after any incomplete install. No model/runtime copies.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import sqlite3
import sys
sys.path.insert(0,str(Path(__file__).resolve().parent))
from storage_optimize import file_hash, session_lease, write_state, emit

LEDGER='illutag.storage-switch.json'
ROLLBACK='illutag.schema2.rollback.sqlite'
PROGRAM_BACKUP='storage-program-rollback'
FILES=('illutag.exe','release-manifest.json','deployment-manifest.json','model-manifest.json','README-portable.txt')

def fault(root,point):
    if os.environ.get('ILLUTAG_SWITCH_TEST_FAULT')==point:
        if not root.is_relative_to(Path('D:/Ai/illuTag-storage-work')):
            raise RuntimeError('Fault injection is restricted to the isolated workspace')
        os._exit(97)

def checked_root(value):
    root=Path(os.path.abspath(value))
    for p in (root,*root.parents):
        if p.exists() and getattr(p.lstat(),'st_file_attributes',0)&0x400:
            raise RuntimeError('Links are forbidden in cutover paths: '+str(p))
    if not root.is_dir(): raise RuntimeError('Existing installation directory required')
    return root

def version(path):
    db=sqlite3.connect(path.resolve().as_uri()+'?mode=ro',uri=True)
    try:
        return db.execute('PRAGMA user_version').fetchone()[0]
    finally:
        db.close()

def refuse_sidecars(path):
    for suffix in ('-wal','-journal'):
        f=Path(str(path)+suffix)
        if f.exists() and f.stat().st_size: raise RuntimeError('Unsettled SQLite journal: '+str(f))

def prepare(root,target,exe):
    target=checked_root(target.parent)/target.name
    exe=checked_root(exe.parent)/exe.name
    state_path=root/LEDGER
    if state_path.exists() or (root/ROLLBACK).exists() or (root/PROGRAM_BACKUP).exists():
        raise RuntimeError('A previous switch must be recovered or finalized first')
    if target.drive.lower()!=root.drive.lower(): raise RuntimeError('Target must be on the same volume')
    original=root/'illutag.sqlite'
    refuse_sidecars(original);refuse_sidecars(target)
    record=json.loads(Path(str(target)+'.verified.json').read_text(encoding='utf-8'))
    if record['sourceSha256']!=file_hash(original) or record['targetSha256']!=file_hash(target):
        raise RuntimeError('Verified source/target identity changed')
    if version(original)!=2 or version(target)!=3: raise RuntimeError('Expected schema 2 -> 3')
    backup=root/PROGRAM_BACKUP
    hashes={name:file_hash(root/name) for name in FILES if (root/name).is_file()}
    staged=root/'illutag.schema3.staging.sqlite'
    if staged.exists(): raise RuntimeError('Unexpected staged DB already exists')
    state=dict(phase='preparing',originalSha256=record['sourceSha256'],newSha256=record['targetSha256'],
        staged=str(staged),sourceTarget=str(target),newExe=str(exe),newExeSha256=file_hash(exe),
        oldProgram=hashes,record=record)
    write_state(state_path,state)
    fault(root,'beforeBackup')
    backup.mkdir()
    for name in hashes: shutil.copy2(root/name,backup/name)
    fault(root,'beforeStaging')
    os.rename(target,staged)
    fault(root,'afterStaging')
    state['phase']='prepared';write_state(state_path,state)
    emit('switch_prepared',bytes=record['bytes'])

def install(root):
    path=root/LEDGER;state=json.loads(path.read_text(encoding='utf-8'))
    if state['phase']!='prepared': raise RuntimeError('Switch must be prepared')
    original=root/'illutag.sqlite';staged=root/'illutag.schema3.staging.sqlite'
    if file_hash(original)!=state['originalSha256'] or file_hash(staged)!=state['newSha256']:
        raise RuntimeError('Switch database hash mismatch')
    os.rename(original,root/ROLLBACK)
    fault(root,'afterOldRename')
    state['phase']='oldSaved';write_state(path,state)
    os.rename(staged,original)
    fault(root,'afterNewRename')
    state['phase']='newInstalled';write_state(path,state)
    exe=Path(state['newExe'])
    if file_hash(exe)!=state['newExeSha256']: raise RuntimeError('Candidate executable changed')
    staged_exe=root/'illutag.storage-upgrade.exe'
    shutil.copy2(exe,staged_exe)
    os.replace(staged_exe,root/'illutag.exe')
    fault(root,'afterExeReplace')
    emit('switch_installed',schema=3)

def recover(root):
    path=root/LEDGER;state=json.loads(path.read_text(encoding='utf-8'))
    if state['phase']=='accepted': raise RuntimeError('Accepted live writes must not be rolled back')
    original=root/'illutag.sqlite';old=root/ROLLBACK
    if old.exists():
        if file_hash(old)!=state['originalSha256']: raise RuntimeError('Original rollback identity mismatch')
        if original.exists():
            if file_hash(original)!=state['newSha256']: raise RuntimeError('Current DB changed; refusing rollback')
            rejected=root/'illutag.schema3.rejected.sqlite'
            if rejected.exists(): raise RuntimeError('Rejected target already exists')
            os.rename(original,rejected)
        os.rename(old,original)
    elif not original.exists() or file_hash(original)!=state['originalSha256']:
        raise RuntimeError('Cannot identify the original database safely')
    for name,digest in state['oldProgram'].items():
        saved=root/PROGRAM_BACKUP/name
        if saved.exists():
            if file_hash(saved)!=digest: raise RuntimeError('Rollback program identity mismatch: '+name)
            shutil.copy2(saved,root/name)
        elif file_hash(root/name)!=digest:
            raise RuntimeError('Missing rollback program: '+name)
    state['phase']='rolledBack';write_state(path,state)
    emit('switch_recovered',schema=version(original))

def accept(root):
    path=root/LEDGER;state=json.loads(path.read_text(encoding='utf-8'))
    if state['phase']!='newInstalled': raise RuntimeError('Only a tested new installation can be accepted')
    if file_hash(root/'illutag.sqlite')!=state['newSha256']: raise RuntimeError('Readonly validation changed the database')
    if file_hash(root/'illutag.exe')!=state['newExeSha256']: raise RuntimeError('Executable mismatch')
    state['phase']='accepted';write_state(path,state)
    emit('switch_accepted',schema=3)

def main():
    p=argparse.ArgumentParser();p.add_argument('action',choices=['prepare','install','recover','accept'])
    p.add_argument('--root',required=True);p.add_argument('--target');p.add_argument('--exe')
    args=p.parse_args();root=checked_root(args.root)
    with session_lease(root):
        if args.action=='prepare':
            if not args.target or not args.exe: raise RuntimeError('Explicit target and executable required')
            prepare(root,Path(args.target),Path(args.exe))
        else: {'install':install,'recover':recover,'accept':accept}[args.action](root)

if __name__=='__main__': main()
