// Old sign-in links are removed from the address, unused (src/linkParams.ts).
// Run: node --test tests/linkParams.test.ts
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { stripSessionLinkParams } from '../src/linkParams.ts';

test('an address without a sign-in link is left alone', () => {
  assert.equal(stripSessionLinkParams('/', '', ''), null);
  assert.equal(stripSessionLinkParams('/', '?lang=vi', '#settings'), null);
});

test('a #session= link is removed', () => {
  assert.equal(stripSessionLinkParams('/', '', '#session=eyJ.x.y&user=bob&connect=h1&name=PC'), '/');
  assert.equal(stripSessionLinkParams('/', '?lang=vi', '#session=t&other=1'), '/?lang=vi#other=1');
});

test('a ?token=&host_id= link is removed with its stream settings', () => {
  assert.equal(stripSessionLinkParams('/', '?token=t&host_id=h&res=1080p&fps=60&bitrate=1&codec=h264&codec_support=3&host_name=x', ''), '/');
  assert.equal(stripSessionLinkParams('/app', '?token=t&keep=1', '#x'), '/app?keep=1#x');
  assert.equal(stripSessionLinkParams('/', '?host_id=h', ''), '/');
});
