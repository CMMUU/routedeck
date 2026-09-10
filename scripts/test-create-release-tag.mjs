import {test} from 'node:test';
import assert from 'node:assert/strict';
import {ensureTag} from './create-release-tag.mjs';
const commit='a'.repeat(40), tag='v0.7.7';
function api(existing, repoId=1355770287) {
  const writes=[];
  return {writes, request:async (url, options={})=>{
    if(url.endsWith('/serylane'))return Response.json({id:repoId});
    if(options.method==='POST'){
      const body=JSON.parse(options.body);writes.push(body);
      assert.deepEqual(body,{ref:`refs/tags/${tag}`,sha:commit});
      existing={type:'commit',sha:commit};return Response.json({}, {status:201});
    }
    return existing ? Response.json({object:existing}) : new Response(null,{status:404});
  }};
}
test('creates only the exact verified missing tag and is idempotent', async()=>{
  const a=api();assert.equal(await ensureTag(tag,commit,a.request),'created');
  assert.equal(await ensureTag(tag,commit,a.request),'existing');assert.equal(a.writes.length,1);
});
test('existing different or annotated tags and wrong repository are never changed', async()=>{
  for(const a of [api({type:'commit',sha:'b'.repeat(40)}),api({type:'tag',sha:commit}),api(null,1)]){
    await assert.rejects(ensureTag(tag,commit,a.request));assert.equal(a.writes.length,0);
  }
});
test('invalid inputs and uncertain reads cannot create tags', async()=>{
  let calls=0;const request=async()=>{calls++;return new Response(null,{status:503});};
  await assert.rejects(ensureTag('../main',commit,request));assert.equal(calls,0);
  await assert.rejects(ensureTag(tag,commit,request));assert.equal(calls,1);
});
