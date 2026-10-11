import {chromium} from 'playwright';
import assert from 'node:assert/strict';
import fs from 'node:fs';
const browser=await chromium.launch({channel:'chrome',headless:true});
const page=await browser.newPage({viewport:{width:1071,height:806}}),checks=[];
try{
 await page.goto('http://127.0.0.1:1421/?preview=1');await page.waitForSelector('.image-tile');
 for(const width of [500,768,1071,1280,1920]){
  await page.setViewportSize({width,height:806});
  for(const immersive of [false,true]){
   if(immersive)await page.getByRole('button',{name:'沉浸浏览',exact:true}).click();
   if(!await page.getByRole('button',{name:'展开导航',exact:true}).isVisible())await page.getByRole('button',{name:'收起导航',exact:true}).click();
   await page.getByRole('button',{name:'展开导航',exact:true}).click();
   const state=await page.locator('.sidebar').evaluate(n=>{const r=n.getBoundingClientRect(),workspace=document.querySelector('.workspace').getBoundingClientRect(),scrim=document.querySelector('.drawer-scrim').getBoundingClientRect();return {background:getComputedStyle(n).backgroundColor,top:r.top,workspaceTop:workspace.top,scrimTop:scrim.top,hit:n.contains(document.elementFromPoint(r.left+30,r.top+200))}});
   assert.equal(state.background,'rgb(224, 229, 236)');assert.equal(state.top,state.workspaceTop);assert.equal(state.scrimTop,state.workspaceTop);assert.equal(state.hit,true);
   assert.equal(await page.locator('.sidebar').getByRole('button',{name:'标签',exact:true}).isVisible(),true);
   if(width===1071&&immersive)await page.screenshot({path:'output/playwright/ui-immersive-drawer-fixed.png'});
   await page.keyboard.press('Escape');assert.equal(await page.locator('.drawer-scrim').count(),0);assert.equal(await page.locator('.image-tile').count()>0,true);
   if(immersive)await page.getByRole('button',{name:'退出沉浸',exact:true}).click();
   checks.push({width,immersive,...state});
  }
 }
 fs.writeFileSync('output/playwright/drawer-qa.json',JSON.stringify({passed:true,checks},null,2));console.log('Drawer QA passed: 10 width/mode combinations');
}finally{await browser.close();}
