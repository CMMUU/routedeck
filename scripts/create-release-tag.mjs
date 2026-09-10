// Called only after six bundle/installer jobs succeed. Never move an existing tag.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';
import {root, verifyVersions} from './version.mjs';
const base = 'https://api.github.com/repos/CMMUU/serylane';
export async function ensureTag(tag, commit, request) {
  assert.match(tag, /^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/);
  assert.match(commit, /^[a-f0-9]{40}$/);
  const repo = await request(base);
  assert.equal(repo.status, 200);
  assert.equal((await repo.json()).id, 1355770287);
  const path = `${base}/git/ref/tags/${tag}`;
  const before = await request(path);
  if (before.status === 200) {
    const existing = await before.json();
    assert.equal(existing.object.type, 'commit', 'Existing annotated tag requires the existing-tag release flow');
    assert.equal(existing.object.sha, commit, 'Refusing to move an existing release tag');
    return 'existing';
  }
  assert.equal(before.status, 404, 'Cannot safely determine whether the tag exists');
  const result = await request(`${base}/git/refs`, {method:'POST', body:JSON.stringify({ref:`refs/tags/${tag}`, sha:commit})});
  assert.equal(result.status, 201, 'Tag creation was not confirmed; inspect before retrying');
  const after = await request(path);
  assert.equal(after.status, 200);
  const tagRef = await after.json();
  assert.equal(tagRef.object.type, 'commit');
  assert.equal(tagRef.object.sha, commit);
  return 'created';
}
async function main() {
  assert.equal(process.env.GITHUB_REPOSITORY, 'CMMUU/serylane');
  assert.equal(process.env.GITHUB_EVENT_NAME, 'workflow_dispatch');
  assert.equal(process.env.GITHUB_REF, 'refs/heads/main');
  const tag = process.env.RELEASE_TAG, commit = process.env.RELEASE_COMMIT;
  verifyVersions(root, tag);
  assert.equal(execFileSync('git',['rev-parse','HEAD'],{cwd:root,encoding:'utf8',windowsHide:true}).trim(), commit);
  assert.ok(process.env.GH_TOKEN);
  const request = (url, options={}) => fetch(url, {...options, redirect:'error', signal:AbortSignal.timeout(30000),
    headers:{Authorization:`Bearer ${process.env.GH_TOKEN}`,Accept:'application/vnd.github+json','Content-Type':'application/json','User-Agent':'Serylane-Release'}});
  console.log(`Release tag ${tag}: ${await ensureTag(tag, commit, request)}; immutable source ${commit}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(()=>{console.error('Release tag verification failed; no tag was moved and no write was retried.');process.exitCode=1;});
}
