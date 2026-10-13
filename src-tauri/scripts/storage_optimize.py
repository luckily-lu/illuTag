"""Offline, lossless schema-2 -> schema-3 conversion. Stdlib only; no model inference.

All output paths must be explicit. The source is opened read-only. A target is never
overwritten. Full migrations require the same exclusive session lease as the app.
"""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import struct
import time

GIB = 1024 ** 3
EXCLUDED_FILE_KEYS = set()
CHILDREN = (
    'image_tags', 'image_ai_state', 'image_user_folders', 'image_user_custom_tags',
    'image_user_supplement_tags', 'image_thumbnails', 'image_atmosphere_signatures',
    'image_color_signatures', 'reference_board_items',
)
RELATIONS = CHILDREN + ('image_auto_tags', 'image_clip_embeddings')

def emit(phase, **fields):
    print(json.dumps(dict(phase=phase, **fields), ensure_ascii=False), flush=True)

def readonly(path):
    db = sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)
    db.execute('PRAGMA query_only=ON')
    return db

def identifier(name):
    return '"' + name.replace('"', '""') + '"'

def columns(db, table):
    return [r[1] for r in db.execute('PRAGMA table_info(' + identifier(table) + ')')]

def schema_files(root):
    root = Path(root)
    return [(root / ('compact_' + name + '.sql')).read_text(encoding='utf-8') for name in ('schema', 'indexes')]

@contextlib.contextmanager
def session_lease(directory):
    if os.name != 'nt':
        raise RuntimeError('Full production migration currently requires the Windows session lease')
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.CreateFileW.argtypes = [ctypes.c_wchar_p, ctypes.c_uint32, ctypes.c_uint32,
                                 ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p]
    kernel.CreateFileW.restype = ctypes.c_void_p
    kernel.CloseHandle.argtypes = [ctypes.c_void_p]
    handle = kernel.CreateFileW(str(Path(directory) / 'illutag.session.lock'),
                               0xC0000000, 0, None, 4, 0x80, None)
    if handle == ctypes.c_void_p(-1).value:
        raise RuntimeError('Close illuTag before migration: exclusive data lease unavailable')
    try:
        yield
    finally:
        kernel.CloseHandle(handle)

def file_hash(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(8 * 1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()

def write_state(path, value):
    staged=Path(str(path)+'.writing-'+str(os.getpid())+'-'+str(time.time_ns()))
    with open(staged,'x',encoding='utf-8') as f:
        json.dump(value,f,ensure_ascii=False,indent=2)
        f.flush();os.fsync(f.fileno())
    os.replace(staged,path)

def footprint(directory):
    total = 0
    seen = set()
    for base, dirs, files in os.walk(directory, followlinks=False):
        if any(Path(base, d).is_symlink() for d in dirs):
            raise RuntimeError('Workspace contains a directory link')
        for name in files:
            try:
                info=Path(base,name).stat()
                key=(info.st_dev,info.st_ino)
                if key not in seen and key not in EXCLUDED_FILE_KEYS:
                    total += info.st_size
                    seen.add(key)
            except FileNotFoundError:
                pass  # A committed transaction can remove its journal during measurement.
    return total

def guard_space(workspace, budget):
    used = footprint(workspace)
    if used >= budget:
        raise RuntimeError(f'Workspace budget reached: {used} >= {budget} bytes')
    if __import__('shutil').disk_usage(workspace).free < 5 * GIB:
        raise RuntimeError('D: free-space reserve reached')

def sample(source, target, limit, workspace):
    src = readonly(source)
    src.execute('BEGIN')
    out = sqlite3.connect(target, uri=True)
    out.execute('PRAGMA journal_mode=DELETE')
    out.execute('PRAGMA cache_size=-65536')
    out.execute('ATTACH DATABASE ? AS old', (Path(source).resolve().as_uri() + '?mode=ro',))
    definitions = list(src.execute("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type,name"))
    for kind, name, table, sql in definitions:
        if kind == 'table': out.execute(sql)
    with out:
        cols = columns(src, 'images')
        cs = ','.join(map(identifier, cols))
        out.execute(f'INSERT INTO images(rowid,{cs}) SELECT rowid,{cs} FROM old.images ORDER BY rowid LIMIT ?', (limit,))
        for kind, table, _, _ in definitions:
            if kind != 'table' or table == 'images': continue
            if table in RELATIONS:
                cs = ','.join('s.' + identifier(c) for c in columns(src, table))
                out.execute(f'INSERT INTO {identifier(table)} SELECT {cs} FROM images i CROSS JOIN old.{identifier(table)} s ON s.image_id=i.id')
            else:
                out.execute(f'INSERT INTO {identifier(table)} SELECT * FROM old.{identifier(table)}')
            guard_space(workspace, 18 * GIB)
    for kind, _, _, sql in definitions:
        if kind == 'index': out.execute(sql)
    out.execute('PRAGMA user_version=2')
    out.commit()
    out.execute('ANALYZE main')
    out.close()
    src.close()
    emit('sample_complete', bytes=Path(target).stat().st_size, images=limit)

def equal_records(old, new, table, low, high):
    cols = columns(old, table)
    cs = ','.join('s.' + identifier(c) for c in cols)
    params = (low, high)
    expected = list(new.execute(f'SELECT {cs} FROM images i CROSS JOIN old.{identifier(table)} s ON s.image_id=i.id WHERE i.image_pk>? AND i.image_pk<=?', params))
    actual = list(new.execute(f'SELECT {cs} FROM images i CROSS JOIN {identifier(table)} s ON s.image_id=i.id WHERE i.image_pk>? AND i.image_pk<=?', params))
    # Keys can include nullable historical fields: typed canonical bytes avoid coercion.
    def canonical(row):
        b = bytearray()
        for v in row:
            if v is None: b += b'N'
            elif isinstance(v, int): b += b'I' + struct.pack('>q', v)
            elif isinstance(v, float): b += b'F' + struct.pack('>d', v)
            else:
                data = v if isinstance(v, bytes) else v.encode('utf-8')
                b += (b'B' if isinstance(v, bytes) else b'T') + struct.pack('>Q', len(data)) + data
        return bytes(b)
    a = sorted(map(canonical, expected))
    b = sorted(map(canonical, actual))
    if a != b: raise RuntimeError(f'Lossless validation failed: {table}, images {low}..{high}')
    return len(expected)

def convert(source, target, schema_root, page_size, workspace, batch_size=1000, max_target_bytes=10*GIB):
    started = time.monotonic()
    schema, indexes = schema_files(schema_root)
    old = readonly(source)
    if old.execute('PRAGMA user_version').fetchone()[0] not in (1,2):
        raise RuntimeError('Conversion expects schema 1 or 2')
    old.execute('BEGIN')
    out = sqlite3.connect(target, uri=True)
    out.execute(f'PRAGMA page_size={page_size}')
    out.execute(f'PRAGMA max_page_count={max_target_bytes // page_size}')
    out.execute('PRAGMA journal_mode=DELETE')
    out.execute('PRAGMA synchronous=FULL')
    out.execute('PRAGMA cache_size=-65536')
    out.execute('PRAGMA temp_store=FILE')
    out.execute('PRAGMA foreign_keys=ON')
    out.execute('ATTACH DATABASE ? AS old', (Path(source).resolve().as_uri() + '?mode=ro',))
    out.executescript(schema)
    source_tables = [r[0] for r in old.execute("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
    # Roots first; foreign keys are checked explicitly after all related roots exist.
    out.execute('PRAGMA foreign_keys=OFF')
    for table in source_tables:
        if table in RELATIONS or table == 'images': continue
        with out: out.execute(f'INSERT INTO {identifier(table)} SELECT * FROM old.{identifier(table)}')
    count, last = old.execute('SELECT COUNT(*),COALESCE(MAX(rowid),0) FROM images').fetchone()
    cols = columns(old, 'images')
    cs = ','.join(map(identifier, cols))
    at = 0
    while at < last:
        high = old.execute('SELECT MAX(rowid) FROM (SELECT rowid FROM images WHERE rowid>? ORDER BY rowid LIMIT ?)', (at,batch_size)).fetchone()[0]
        if high is None: break
        with out:
            out.execute(f'INSERT INTO images(image_pk,{cs}) SELECT rowid,{cs} FROM old.images WHERE rowid>? AND rowid<=?', (at,high))
        if list(old.execute(f'SELECT {cs} FROM images WHERE rowid>? AND rowid<=? ORDER BY rowid',(at,high))) != list(out.execute(f'SELECT {cs} FROM images WHERE image_pk>? AND image_pk<=? ORDER BY image_pk',(at,high))):
            raise RuntimeError('Image metadata validation failed')
        at = high
        guard_space(workspace,18*GIB)
    totals = {name:0 for name in RELATIONS}
    at = 0
    report_at = 0
    while at < last:
        high = out.execute('SELECT MAX(image_pk) FROM (SELECT image_pk FROM images WHERE image_pk>? ORDER BY image_pk LIMIT ?)', (at,batch_size)).fetchone()[0]
        if high is None: break
        params = (at,high)
        scope = 'FROM images i CROSS JOIN old.image_auto_tags a ON a.image_id=i.id WHERE i.image_pk>? AND i.image_pk<=?'
        with out:
            for column in ('model_name','category','tag_en','tag_zh'):
                out.execute(f'INSERT OR IGNORE INTO storage_strings(value) SELECT DISTINCT a.{column} {scope} AND a.{column} IS NOT NULL', params)
            out.execute('''INSERT INTO storage_auto_tags
              SELECT i.image_pk,m.string_pk,c.string_pk,t.string_pk,z.string_pk,a.confidence,a.updated_at
              FROM images i CROSS JOIN old.image_auto_tags a ON a.image_id=i.id
              JOIN storage_strings m ON m.value=a.model_name JOIN storage_strings c ON c.value=a.category
              JOIN storage_strings t ON t.value=a.tag_en LEFT JOIN storage_strings z ON z.value=a.tag_zh
              WHERE i.image_pk>? AND i.image_pk<=?''', params)
            out.execute('''INSERT OR IGNORE INTO storage_clip_models(model_id,model_version)
              SELECT DISTINCT e.model_id,e.model_version FROM images i CROSS JOIN old.image_clip_embeddings e ON e.image_id=i.id
              WHERE i.image_pk>? AND i.image_pk<=?''', params)
            out.execute('''INSERT INTO storage_clip_embeddings(image_id,model_pk,dimension,normalized,vector_blob,source_modified_at,updated_at)
              SELECT i.image_pk,m.model_pk,e.dimension,e.normalized,e.vector_blob,e.source_modified_at,e.updated_at
              FROM images i CROSS JOIN old.image_clip_embeddings e ON e.image_id=i.id
              JOIN storage_clip_models m ON m.model_id=e.model_id AND m.model_version=e.model_version
              WHERE i.image_pk>? AND i.image_pk<=?''', params)
            for table in CHILDREN:
                cols = columns(old,table)
                cs = ','.join('i.image_pk' if c=='image_id' else 's.'+identifier(c) for c in cols)
                out.execute(f'INSERT INTO storage_{table} SELECT {cs} FROM images i CROSS JOIN old.{table} s ON s.image_id=i.id WHERE i.image_pk>? AND i.image_pk<=?', params)
        for table in RELATIONS:
            totals[table] += equal_records(old,out,table,at,high)
        at = high
        guard_space(workspace,18*GIB)
        if time.monotonic()-report_at >= 5:
            emit('copy_verified', image_key=at, total_images=count, bytes=Path(target).stat().st_size, elapsed=round(time.monotonic()-started,1))
            report_at=time.monotonic()
    # Detect any orphan source records instead of silently dropping them.
    for table in RELATIONS:
        original = old.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0]
        if original != totals[table]: raise RuntimeError(f'Orphan or missing records: {table}: {original} != {totals[table]}')
    for table in source_tables:
        if table in RELATIONS or table=='images': continue
        if list(old.execute(f'SELECT * FROM {identifier(table)} ORDER BY rowid')) != list(out.execute(f'SELECT * FROM {identifier(table)} ORDER BY rowid')):
            raise RuntimeError('Metadata mismatch: '+table)
    for statement in indexes.split('\n\n'):
        if statement.strip():
            out.executescript(statement)
            guard_space(workspace,18*GIB)
    out.execute('PRAGMA user_version=3')
    out.commit()
    out.execute('PRAGMA foreign_keys=ON')
    if out.execute('PRAGMA foreign_key_check').fetchone(): raise RuntimeError('Foreign key validation failed')
    checks=list(out.execute('PRAGMA integrity_check'))
    if checks != [('ok',)]: raise RuntimeError('Integrity validation failed: '+repr(checks[:5]))
    out.execute('ANALYZE main')
    out.close();old.close()
    emit('conversion_complete', images=count, rows=totals, bytes=Path(target).stat().st_size, elapsed=round(time.monotonic()-started,1))

def main():
    p=argparse.ArgumentParser()
    p.add_argument('action',choices=['sample','convert'])
    p.add_argument('--source',required=True);p.add_argument('--target',required=True)
    p.add_argument('--workspace',required=True);p.add_argument('--schema-root',required=True)
    p.add_argument('--images',type=int,default=20000)
    p.add_argument('--page-size',type=int,choices=[4096,8192,16384],default=16384)
    p.add_argument('--max-target-mib',type=int,default=10240)
    p.add_argument('--production',action='store_true')
    p.add_argument('--allow-other-drive',action='store_true',help='For an explicitly selected restore destination')
    args=p.parse_args()
    if not 1<=args.max_target_mib<=10240: raise RuntimeError('Target budget must be 1..10240 MiB')
    source=Path(args.source).resolve();target=Path(args.target).resolve();workspace=Path(args.workspace).resolve()
    if not source.is_relative_to(workspace):
        info=source.stat();EXCLUDED_FILE_KEYS.add((info.st_dev,info.st_ino))
    if not workspace.is_dir() or (workspace.drive.upper()!='D:' and not args.allow_other_drive): raise RuntimeError('Explicit existing D: workspace required')
    if not target.is_relative_to(workspace) or target.exists(): raise RuntimeError('Target must be a new file inside the explicit workspace')
    if source==target or source.is_relative_to(target): raise RuntimeError('Invalid source/target')
    temp=workspace/'sqlite-temp';temp.mkdir(exist_ok=True)
    os.environ['TEMP']=os.environ['TMP']=str(temp)
    lease=session_lease(source.parent) if args.production else contextlib.nullcontext()
    with lease:
        guard_space(workspace,18*GIB)
        before=file_hash(source) if args.production else None
        if args.action=='sample': sample(source,target,args.images,workspace)
        else: convert(source,target,args.schema_root,args.page_size,workspace,max_target_bytes=args.max_target_mib*1024**2)
        if args.production:
            if file_hash(source)!=before: raise RuntimeError('Source changed; target must not be deployed')
            with readonly(target) as db:
                image_count=db.execute('SELECT COUNT(*) FROM images').fetchone()[0]
                tag_count=db.execute('SELECT COUNT(*) FROM storage_auto_tags').fetchone()[0]
                vector_count=db.execute('SELECT COUNT(*) FROM storage_clip_embeddings').fetchone()[0]
            state=dict(source=str(source),sourceSha256=before,targetSha256=file_hash(target),
                schemaVersion=3,images=image_count,tags=tag_count,vectors=vector_count,bytes=target.stat().st_size)
            write_state(Path(str(target)+'.verified.json'),state)
            emit('source_and_target_verified',**state)

if __name__=='__main__': main()
