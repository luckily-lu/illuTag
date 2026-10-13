"""Real process-exit faults at every critical rename; tiny isolated fixtures only."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parent))
from storage_optimize import file_hash,write_state

ROOT=Path(os.environ['ILLUTAG_STORAGE_TEST_ROOT']).resolve()
if not ROOT.is_relative_to(Path('D:/Ai/illuTag-storage-work')):
    raise RuntimeError('Explicit isolated test root required')
ROOT.mkdir(exist_ok=True)
SCRIPT=Path(__file__).with_name('storage_cutover.py')

class CutoverTests(unittest.TestCase):
    def fixture(self,directory):
        root=directory/'installation';root.mkdir()
        original=root/'illutag.sqlite'
        target=directory/'target.sqlite'
        for path,version in ((original,2),(target,3)):
            with sqlite3.connect(path) as db:
                db.execute('CREATE TABLE images(id TEXT PRIMARY KEY,path TEXT)')
                db.execute("INSERT INTO images VALUES('original-id','D:/original.png')")
                db.execute('PRAGMA user_version='+str(version))
            db.close()
        (root/'illutag.exe').write_bytes(b'original executable')
        (root/'release-manifest.json').write_text('{"version":"old"}')
        exe=directory/'candidate.exe';exe.write_bytes(b'candidate executable')
        write_state(Path(str(target)+'.verified.json'),dict(sourceSha256=file_hash(original),targetSha256=file_hash(target),bytes=target.stat().st_size))
        return root,target,exe,file_hash(original)

    def command(self,action,root,*args,fault=None):
        env=dict(os.environ)
        if fault:env['ILLUTAG_SWITCH_TEST_FAULT']=fault
        else:env.pop('ILLUTAG_SWITCH_TEST_FAULT',None)
        return subprocess.run([sys.executable,'-B',str(SCRIPT),action,'--root',str(root),*args],env=env,capture_output=True,timeout=20)

    def test_every_crash_point_recovers_original_database_and_program(self):
        for point in ('beforeBackup','beforeStaging','afterStaging','afterOldRename','afterNewRename','afterExeReplace'):
            with self.subTest(point=point),tempfile.TemporaryDirectory(dir=ROOT,prefix='fault-') as folder:
                root,target,exe,digest=self.fixture(Path(folder))
                prepare=self.command('prepare',root,'--target',str(target),'--exe',str(exe),fault=point)
                if point in ('beforeBackup','beforeStaging','afterStaging'):
                    self.assertEqual(prepare.returncode,97,prepare.stderr)
                else:
                    self.assertEqual(prepare.returncode,0,prepare.stderr)
                    install=self.command('install',root,fault=point)
                    self.assertEqual(install.returncode,97,install.stderr)
                recover=self.command('recover',root)
                self.assertEqual(recover.returncode,0,recover.stderr)
                self.assertEqual(file_hash(root/'illutag.sqlite'),digest)
                self.assertEqual((root/'illutag.exe').read_bytes(),b'original executable')

    def test_acceptance_prevents_rollback_of_new_live_writes(self):
        with tempfile.TemporaryDirectory(dir=ROOT,prefix='accepted-') as folder:
            root,target,exe,_=self.fixture(Path(folder))
            for action,args in (('prepare',('--target',str(target),'--exe',str(exe))),('install',()),('accept',())):
                result=self.command(action,root,*args);self.assertEqual(result.returncode,0,result.stderr)
            self.assertNotEqual(self.command('recover',root).returncode,0)
            self.assertEqual((root/'illutag.exe').read_bytes(),b'candidate executable')

if __name__=='__main__':unittest.main()
