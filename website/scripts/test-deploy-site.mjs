import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import { readFile } from 'node:fs/promises';
import { retryRead, sourceAssetBytes, uploadedVersion, verifyStatic } from './deploy-site.mjs';

test('robots verification preserves Cloudflare managed policy and checks the full source portion', async () => {
  const owned = 'User-agent: *\nAllow: /\n\nSitemap: https://serylane.cmmuu.com/sitemap.xml\n';
  const prefix = '# As a condition of accessing this website, you agree\n\n# BEGIN Cloudflare Managed content\nUser-agent: GPTBot\nDisallow: /\n\n# END Cloudflare Managed Content\n\n';
  const bytes = Buffer.from(prefix + owned);
  assert.equal(sourceAssetBytes('/robots.txt', bytes).toString(), owned);
  assert.equal(sourceAssetBytes('/site.js', bytes), bytes);
  assert.equal(sourceAssetBytes('/robots.txt', Buffer.from(owned)).toString(), owned);
  assert.throws(() => sourceAssetBytes('/robots.txt', Buffer.from(prefix + prefix + owned)));
  const expected = {files:{'/robots.txt':createHash('sha256').update(owned).digest('hex')}};
  const fetcher = body => async url => url.endsWith('/build-info.json')
    ? Response.json(expected, {headers:{'cache-control':'no-store'}}) : new Response(body);
  await verifyStatic(expected, fetcher(prefix + owned));
  await assert.rejects(verifyStatic(expected, fetcher(prefix + owned.replace('Allow: /', 'Disallow: /'))));
});

test('edge propagation checks retry only reads and remain bounded', async () => {
  let reads = 0, waits = 0;
  const result = await retryRead(async () => {
    if (++reads < 4) throw new Error('Previous edge manifest');
    return 'verified';
  }, async () => { waits++; });
  assert.equal(result, 'verified');
  assert.equal(reads, 4);
  assert.equal(waits, 3);
  reads = 0; waits = 0;
  await assert.rejects(retryRead(async () => { reads++; throw new Error('Still unavailable'); }, async () => { waits++; }));
  assert.equal(reads, 6);
  assert.equal(waits, 5);
});
test('CI invokes the deployment script, not pnpm workspace deploy', async () => {
  const workflow = await readFile(new URL('../../.github/workflows/website.yml', import.meta.url), 'utf8');
  assert.match(workflow, /run: pnpm run deploy/);
  assert.doesNotMatch(workflow, /run: pnpm deploy\s*$/m);
});
test('upload parser rejects missing, duplicate and malformed IDs', () => {
  const id = 'e032654d-5513-40f7-96db-6abbac00b55b';
  assert.equal(uploadedVersion(`Uploaded assets\nWorker Version ID: ${id}\n`), id);
  for (const output of ['', `Current Version ID: ${id}`, `Worker Version ID: ${id}\nWorker Version ID: ${id}`, 'Worker Version ID: ' + '-'.repeat(36)]) {
    assert.throws(() => uploadedVersion(output));
  }
});
test('online verification requires marker AND actual bytes, never a marker alone', async () => {
  const hash = createHash('sha256').update('expected').digest('hex');
  const expected = { sourceCommit: 'a'.repeat(40), websiteTree: 'b'.repeat(40), files: { '/': hash } };
  const fetcher = (body, marker = expected, cache = 'no-store') => async url => url.endsWith('/build-info.json')
    ? Response.json(marker, { headers: { 'cache-control': cache } }) : new Response(body);
  await verifyStatic(expected, fetcher('expected'));
  await assert.rejects(verifyStatic(expected, fetcher('stale page')));
  await assert.rejects(verifyStatic(expected, fetcher('expected', { ...expected, sourceCommit: 'c'.repeat(40) })));
  await assert.rejects(verifyStatic(expected, fetcher('expected', expected, 'public')));
});
