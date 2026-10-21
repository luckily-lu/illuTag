// Generated preview data in headless Edge only. No native or production process.
import {createServer} from 'vite';
import {chromium} from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const output=path.resolve(process.argv[2]||'');
assert.ok(process.argv[2]&&path.isAbsolute(process.argv[2]));
assert.ok(output.split(/[\\/]/).some(p=>p.toLowerCase()==='illutag-refactor-work'));
fs.mkdirSync(output,{recursive:true});
const previous=[process.env.TEMP,process.env.TMP];process.env.TEMP=output;process.env.TMP=output;
let server,browser;
async function anchored(page,above=false){
  const t=await page.getByRole('combobox',{name:'候选筛选严格度'}).boundingBox();
  const p=await page.getByRole('listbox',{name:'候选筛选严格度'}).boundingBox();
  assert.ok(t&&p);
  const topGap=p.y-t.y-t.height,bottomGap=t.y-p.y-p.height;
  assert.ok(Math.abs(topGap-8)<2||Math.abs(bottomGap-8)<2,`detached menu gaps: ${topGap}/${bottomGap}`);
  if(above)assert.ok(Math.abs(bottomGap-8)<2,'short menu should attach directly above');
  assert.ok(p.y>=7&&p.y+p.height<=page.viewportSize().height-7,'menu stays in viewport');
}
try{
  server=await createServer({root:fileURLToPath(new URL('../../',import.meta.url)),server:{host:'127.0.0.1',port:0,open:false}});await server.listen();
  const origin=`http://127.0.0.1:${server.httpServer.address().port}`;
  browser=await chromium.launch({channel:'msedge',headless:true,downloadsPath:output});
  for(const width of [1430,500]){
    const context=await browser.newContext({viewport:{width:1430,height:840}});
    try{
      await context.route('**/*',async route=>{
        const u=new URL(route.request().url());
        if(!['127.0.0.1','localhost'].includes(u.hostname)&&!['data:','blob:'].includes(u.protocol))return route.abort();
        if(u.pathname==='/src/shared/ipc.ts'){
          const response=await route.fetch();let body=await response.text();const marker='if (!isTauri()) {';assert.ok(body.includes(marker));
          body=body.replace(marker,'if (command === "duplicate_workflow_status" && !globalThis.__readReleased) await new Promise(resolve => { globalThis.__releaseDuplicateRead=()=>{globalThis.__readReleased=true;resolve();}; });\n'+marker);
          return route.fulfill({response,body});
        }
        return route.continue();
      });
      const page=await context.newPage();await page.goto(`${origin}/?preview=1`);
      await page.getByRole('button',{name:'重复检索',exact:true}).click();
      await page.getByRole('heading',{name:'正在读取检索结果',exact:true}).waitFor();
      assert.equal(await page.getByRole('heading',{name:'从全库检索开始',exact:true}).count(),0);
      await page.evaluate(()=>globalThis.__releaseDuplicateRead());
      const start=page.getByRole('button',{name:'开始全库检索',exact:true});await start.waitFor();assert.ok(await start.isEnabled());await start.click();
      await page.getByRole('heading',{name:'全库检索结果',exact:true}).waitFor();
      await page.setViewportSize({width,height:840});await page.locator('.workflow-heading > button').click();
      const trigger=page.getByRole('combobox',{name:'候选筛选严格度'});await trigger.scrollIntoViewIfNeeded();
      await trigger.evaluate(el=>{const root=el.closest('.duplicate-workflow-page');root.scrollTop+=el.getBoundingClientRect().top-(innerHeight-180);});
      await trigger.click();await page.getByRole('listbox').waitFor();await anchored(page,true);
      await trigger.evaluate(el=>el.closest('.duplicate-workflow-page').scrollTop+=35);
      await page.waitForTimeout(80);await anchored(page);
      const header=await page.locator('.workflow-heading').boundingBox();assert.ok(header.y>=43,'page heading stays below titlebar');
      await page.screenshot({path:path.join(output,`loading-select-${width}.png`)});
      await page.keyboard.press('Escape');assert.equal(await page.getByRole('listbox').count(),0);
      assert.ok(await trigger.evaluate(el=>document.activeElement===el));
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
    }finally{await context.close();}
  }
  console.log('Loading/start and short-menu alignment passed headlessly at 1430/500 CSS px.');
}finally{await browser?.close();await server?.close();process.env.TEMP=previous[0];process.env.TMP=previous[1];}
