import { launch, isolatedPath, sleep } from './native-session.mjs';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
const [exeArg, rootArg, resources] = process.argv.slice(2);
const exe = isolatedPath(exeArg, 'candidate'), root = isolatedPath(rootArg, 'fresh QA root');
assert.ok(!fs.existsSync(root));
assert.ok(path.isAbsolute(resources));
const python = path.join(fs.realpathSync(resources), 'runtime/python/python.exe');
const tags = ['long_hair','blue_sky','outdoors','scenery','cloud','white_dress','beach','sunset','city','1girl','solo','smile','blush','short_hair','blue_eyes','brown_hair','black_hair','looking_at_viewer','standing','sky','tree','water','flower','dress'];
const report = { passed:false, syntheticOnly:true, checks:[] };
let session, failure;
function py(code) {
  return JSON.parse(execFileSync(python,['-B','-c',code,root,JSON.stringify(tags)],{
    windowsHide:true,timeout:30000,env:{...process.env,PYTHONDONTWRITEBYTECODE:'1'},
  }).toString());
}
async function errorFrom(command,args) {
  return session.page.evaluate(async ({command,args})=>{
    try {await window.__TAURI_INTERNALS__.invoke(command,args);return null;}
    catch(error) {return typeof error==='string'?error:JSON.stringify(error);}
  },{command,args});
}
try {
  session = await launch(exe,root,9797,{resourceRoot:resources});
  await session.page.waitForSelector('.app');
  assert.equal((await session.invoke('list_jobs')).length,0);
  report.fixture = py(String.raw`
from PIL import Image,ImageDraw
import pathlib,sys,json
p=pathlib.Path(sys.argv[1]).resolve();assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
for n in range(40):
 size={0:(200,360),1:(360,180),2:(100,1400)}.get(n,(160,200))
 im=Image.new('RGB',size,(60+n*3,130,180));d=ImageDraw.Draw(im);d.ellipse((12+n,15,min(size[0]-2,60+n),65),fill=(220,200-n*2,120))
 if n==2:
  for y in range(100,1400,100):d.rectangle((10,y,90,y+40),fill=(180,200,140))
 im.save(p/'assets'/f'sample-{n:02}.png')
print(json.dumps({'images':40,'bytes':sum(f.stat().st_size for f in (p/'assets').glob('*.png'))}))
`);
  await session.invoke('add_gallery_folder_command',{folderPath:path.join(root,'assets')});
  const job = await session.invoke('start_job',{request:{kind:'scan',thumbnails:false,sourceFolderId:null}});
  const until = Date.now()+60000;
  while (true) {
    const record = (await session.invoke('list_jobs')).find(j=>j.id===job.id);
    if (['completed','failed','partial','cancelled'].includes(record?.state)) { assert.equal(record.state,'completed'); break; }
    assert.ok(Date.now()<until,'Synthetic scan timeout'); await sleep(40);
  }
  py(String.raw`
import sqlite3,pathlib,sys,json
p=pathlib.Path(sys.argv[1]).resolve();assert 'illutag-refactor-work' in [s.lower() for s in p.parts]
c=sqlite3.connect(p/'data'/'illutag.sqlite');c.execute('pragma foreign_keys=on');tags=json.loads(sys.argv[2])
for s in ['wd-swinv2-tagger-v3','general']+tags:c.execute('insert or ignore into storage_strings(value) values(?)',(s,))
pk=lambda s:c.execute('select string_pk from storage_strings where value=?',(s,)).fetchone()[0]
for image,name in c.execute('select image_pk,file_name from images').fetchall():
 for tag in tags:c.execute('insert into storage_auto_tags values(?,?,?,?,NULL,?,0)',(image,pk('wd-swinv2-tagger-v3'),pk('general'),pk(tag),0.9899 if name=='sample-00.png' else 0.995))
c.commit();c.close();print('{}')
`);
  await session.page.reload();
  await session.page.locator('.sidebar').getByRole('button',{name:'标签',exact:true}).click();
  await session.page.locator('.tag-cover-card').first().waitFor();
  await session.page.getByLabel('仅图库内',{exact:true}).check();
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-card-image img').length===18);
  const covers = await session.invoke('list_tag_covers_command',{tags});
  const ids = covers.map(c=>c.image?.id).filter(Boolean);
  assert.equal(ids.length,tags.length);
  assert.equal(new Set(ids).size,ids.length);
  assert.ok(covers.every(c=>c.image.fileName!=='sample-00.png'));
  const low = (await session.invoke('query_tag_preview_command',{tags:['long_hair'],offset:0,limit:50})).images.find(i=>i.fileName==='sample-00.png');
  assert.ok(low,'Ordinary tag browsing must retain low confidence images');
  assert.match(await errorFrom('save_tag_cover_command',{request:{tag:'long_hair',imageId:low.id,x:50,y:50,zoom:1}}),/99%/);
  assert.match(await errorFrom('save_tag_cover_command',{request:{tag:'blue_sky',imageId:covers[0].image.id,x:50,y:50,zoom:1}}),/已用于/);
  const candidates = await session.invoke('query_tag_cover_candidates_command',{tag:'long_hair',offset:0,limit:50});
  assert.ok(candidates.images.every(i=>i.id!==low.id && (!ids.includes(i.id)||i.id===covers[0].image.id)));
  report.checks.push('Global cover uniqueness across pages, 99% filtering and save rejection; ordinary tag results remain unchanged');
  const tagSearch = session.page.getByLabel('搜索标签',{exact:true});
  await tagSearch.fill('long_hair');
  await session.page.locator('.tag-cover-card').filter({hasText:'long_hair'}).locator('.tag-card-main').click();
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-preview-masonry img').length===18);
  const preview = session.page.locator('.tag-results-scroll');
  await preview.hover(); await session.page.mouse.wheel(0,10000);
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-preview-masonry img').length===36);
  await preview.hover(); await session.page.mouse.wheel(0,10000);
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-preview-masonry img').length===40);
  assert.equal(await session.page.getByRole('button',{name:'加载更多图片',exact:true}).count(),0);
  // A new tag query starts from its first page rather than an old scroll offset.
  await session.page.locator('.tag-cover-card').filter({hasText:'long_hair'}).locator('.tag-card-main').click();
  await tagSearch.fill('blue_sky');
  await session.page.locator('.tag-cover-card').filter({hasText:'blue_sky'}).locator('.tag-card-main').click();
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-preview-masonry img').length===18);
  assert.equal(await preview.evaluate(el=>el.scrollTop),0);
  await tagSearch.fill('');
  report.checks.push('Preview auto-loads 18 -> 36 -> 40 images on real wheel scroll with no load button; changed query resets the page and scroll');
  await session.page.setViewportSize({width:1430,height:840});
  const lastFirstPage = (await session.invoke('query_tag_preview_command',{tags:['blue_sky'],offset:0,limit:18})).images.at(-1);
  await session.page.getByRole('button',{name:`查看图片：${lastFirstPage.fileName}`,exact:true}).click();
  const viewer = session.page.getByRole('dialog',{name:'大图查看器',exact:true});
  await viewer.waitFor();
  await viewer.getByRole('button',{name:'下一张',exact:true}).click();
  await session.page.waitForFunction(()=>document.querySelector('.viewer-position').textContent.replace(/\s+/g,'').startsWith('19/'));
  await viewer.getByRole('button',{name:'缩略图',exact:true}).click();
  for (const size of [{width:1430,height:840},{width:1024,height:700},{width:768,height:600},{width:500,height:500}]) {
    await session.page.setViewportSize(size);
    for (const withInfo of [false,true]) {
      const info = viewer.getByRole('button',{name:'信息',exact:true});
      if ((await info.getAttribute('aria-pressed'))!==String(withInfo)) await info.click();
      const geometry = await viewer.evaluate(el=>{
        const rect=s=>el.querySelector(s).getBoundingClientRect();
        const controls=rect('.viewer-controls'),film=rect('.viewer-filmstrip'),stage=rect('.viewer-stage'),row=rect('.viewer-control-row'),root=el.getBoundingClientRect();
        return {controlsBottom:controls.bottom,filmTop:film.top,stageBottom:stage.bottom,rowTop:row.top,stageHeight:stage.height,inside:controls.left>=root.left&&controls.right<=root.right&&film.bottom<=root.bottom+.5,overflow:el.scrollWidth>el.clientWidth};
      });
      assert.ok(geometry.controlsBottom<=geometry.filmTop+.5,JSON.stringify({size,withInfo,geometry}));
      assert.ok(geometry.stageBottom<=geometry.rowTop+.5 && geometry.stageHeight>=48 && geometry.inside && !geometry.overflow,JSON.stringify({size,withInfo,geometry}));
    }
  }
  await session.page.setViewportSize({width:1430,height:840});
  await viewer.getByRole('button',{name:'信息',exact:true}).click();
  await viewer.getByRole('button',{name:'原始大小',exact:true}).click();
  await session.page.waitForFunction(()=>{const i=document.querySelector('.viewer-stage>img');return i.complete&&i.naturalWidth>0&&Math.abs(i.getBoundingClientRect().width-i.naturalWidth)<.5});
  await session.page.keyboard.press('0');
  await session.page.waitForFunction(()=>document.querySelector('.viewer-scale').textContent.includes('适应'));
  await session.page.screenshot({path:path.join(root,'logs/lightbox-filmstrip.png')});
  await viewer.getByRole('button',{name:'更多图片操作',exact:true}).click();
  await session.page.keyboard.press('Escape');
  assert.equal(await viewer.locator('.viewer-more-menu').count(),0);
  assert.equal(await viewer.count(),1);
  await viewer.getByRole('button',{name:'缩略图',exact:true}).click();
  await viewer.getByRole('button',{name:'关闭查看器',exact:true}).click();
  await viewer.waitFor({state:'detached'});
  await session.page.screenshot({path:path.join(root,'logs/tags-results-priority.png')});
  // Exercise horizontal, portrait and very long originals without changing aspect.
  await preview.hover();await session.page.mouse.wheel(0,10000);
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-preview-masonry img').length>=36);
  await preview.hover();await session.page.mouse.wheel(0,10000);
  await session.page.waitForFunction(()=>document.querySelectorAll('.tag-preview-masonry img').length===40);
  for (const name of ['sample-00.png','sample-01.png','sample-02.png']) {
    await session.page.getByRole('button',{name:`查看图片：${name}`,exact:true}).click();await viewer.waitFor();
    await session.page.waitForFunction(()=>{const i=document.querySelector('.viewer-stage>img');if(!i?.naturalWidth)return false;const r=i.getBoundingClientRect(),s=i.parentElement.getBoundingClientRect();return Math.abs(r.width/r.height-i.naturalWidth/i.naturalHeight)<.001&&r.width<=s.width+.5&&r.height<=s.height+.5});
    await viewer.getByRole('button',{name:'关闭查看器',exact:true}).click();await viewer.waitFor({state:'detached'});
  }
  report.checks.push('Immersive viewer: separate zoom and filmstrip rows never overlap at 500/768/1024/1430 widths with info open/closed; true 1:1, keyboard fit, staged Esc and continuous next-page browsing; original aspect retained for landscape/portrait/long images');
  await session.page.setViewportSize({width:1430,height:840});
  const catalog = session.page.locator('.tag-catalog-scroll');
  await catalog.hover(); await session.page.mouse.wheel(0,600);
  await session.page.waitForFunction(()=>document.querySelector('.tag-catalog-scroll').scrollTop>0);
  await session.page.getByRole('button',{name:'下一页',exact:true}).click();
  await session.page.waitForFunction(()=>document.querySelector('.tag-catalog-pagination span').textContent.trim()==='2 / 2');
  assert.equal(await catalog.evaluate(el=>el.scrollTop),0);
  await session.page.getByRole('button',{name:'上一页',exact:true}).click();
  for (const size of [{width:1280,height:500},{width:500,height:800},{width:500,height:500}]) {
    await session.page.setViewportSize(size);
    assert.equal(await session.page.locator('.tag-workbench').evaluate(el=>el.scrollWidth>el.clientWidth),false);
    await session.page.locator('.tag-catalog-pagination').scrollIntoViewIfNeeded();
    assert.ok(await session.page.getByRole('button',{name:'下一页',exact:true}).isVisible());
  }
  report.checks.push('Real wheel scrolling, page reset and reachable pagination at 1430/1280/500 CSS px and low height');
  await session.page.setViewportSize({width:1430,height:840});
  const search = session.page.getByLabel('搜索标签',{exact:true});
  await search.fill('long_hair');
  const hair = session.page.locator('.tag-cover-card').filter({hasText:'long_hair'});
  await hair.locator('.tag-cover-edit').click();
  const editor = session.page.getByRole('dialog',{name:/设置标签封面/});
  await editor.locator('.tag-cover-choices button').first().waitFor();
  assert.ok((await editor.innerText()).includes('99%'));
  await editor.locator('.tag-cover-choices button').last().click();
  await editor.getByRole('button',{name:'固定此封面',exact:true}).click();
  await editor.waitFor({state:'detached'});
  const pin = (await session.invoke('list_tag_covers_command',{tags:['long_hair']}))[0];
  assert.equal(pin.pinned,true);
  await session.page.reload();
  const saved = (await session.invoke('list_tag_covers_command',{tags:['long_hair']}))[0];
  assert.equal(saved.image.id,pin.image.id);
  report.checks.push('Native cover editor shows only eligible choices and retains the fixed unique cover after reload');
  report.passed=true;
} catch(error) { failure=error;report.error=error.message; }
finally {
  if(session) {
    await session.close();report.session=session.getDiagnostics();
    if(report.session.killed||report.session.exitCode!==0){report.passed=false;failure ||= Error('Test application did not close normally');}
  }
  fs.writeFileSync(path.join(root,'logs/tag-cover-policy-qa.json'),JSON.stringify(report,null,2));
  console.log(JSON.stringify(report));
}
if(failure)throw failure;
