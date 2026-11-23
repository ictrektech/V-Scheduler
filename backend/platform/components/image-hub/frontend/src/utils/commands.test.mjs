// utils/commands.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  buildDownloadAndRun,
  buildPullCommands,
  buildPushCommands,
  buildOneLiner,
  buildScriptDownload,
  registryAddress,
  scriptUrlFrom,
  socatForward,
  socatStop,
  localRef,
  buildPlainText,
} from './commands.ts';

test('registryAddress / socatForward', () => {
  assert.equal(registryAddress('192.168.1.100', 19913), '192.168.1.100:19913');
  assert.equal(
    socatForward('192.168.1.100', 19913),
    'socat TCP-LISTEN:19913,fork,reuseaddr TCP:192.168.1.100:19913 &',
  );
});

test('socatStop：按端口精确停止后台 socat 并容忍无匹配', () => {
  assert.equal(socatStop(19913), "pkill -f 'socat TCP-LISTEN:19913' || true");
});

test('localRef：127.0.0.1 免 HTTPS + 保留上游名', () => {
  assert.equal(
    localRef(19913, 'docker.io/foo/bar', 'tag'),
    '127.0.0.1:19913/docker.io/foo/bar:tag',
  );
});

test('buildPullCommands：五步且 retag 还原上游名 + 末步停 socat', () => {
  const steps = buildPullCommands('192.168.1.100', 19913, 'docker.io/foo/bar', 'tag');
  assert.equal(steps.length, 5);
  assert.equal(steps[0].command, 'socat TCP-LISTEN:19913,fork,reuseaddr TCP:192.168.1.100:19913 &');
  assert.equal(steps[1].command, 'docker pull 127.0.0.1:19913/docker.io/foo/bar:tag');
  assert.equal(steps[2].command, 'docker tag 127.0.0.1:19913/docker.io/foo/bar:tag docker.io/foo/bar:tag');
  assert.equal(steps[3].command, 'docker rmi 127.0.0.1:19913/docker.io/foo/bar:tag');
  assert.equal(steps[4].command, "pkill -f 'socat TCP-LISTEN:19913' || true");
});

test('buildPushCommands：四步且推送名保留上游完整名称 + 末步停 socat', () => {
  const steps = buildPushCommands('192.168.1.100', 19913, 'docker.io/foo/bar', 'tag');
  assert.equal(steps.length, 4);
  assert.equal(steps[1].command, 'docker tag docker.io/foo/bar:tag 127.0.0.1:19913/docker.io/foo/bar:tag');
  assert.equal(steps[2].command, 'docker push 127.0.0.1:19913/docker.io/foo/bar:tag');
  assert.equal(steps[3].command, "pkill -f 'socat TCP-LISTEN:19913' || true");
});

test('scriptUrlFrom：hash 路由下解析到 <url-base>/image-hub.sh', () => {
  assert.equal(
    scriptUrlFrom('https://192.168.8.69:1180/app/com.ictrek.image-hub/#/images'),
    'https://192.168.8.69:1180/app/com.ictrek.image-hub/image-hub.sh',
  );
  assert.equal(
    scriptUrlFrom('http://localhost:5189/#/images'),
    'http://localhost:5189/image-hub.sh',
  );
  assert.equal(
    scriptUrlFrom('https://host/app/com.ictrek.image-hub/#/image?repository=docker.io%2Ffoo'),
    'https://host/app/com.ictrek.image-hub/image-hub.sh',
  );
});

test('buildOneLiner：含脚本 URL、地址、子命令与上游镜像名，curl 带 -k', () => {
  assert.equal(
    buildOneLiner(
      'https://192.168.8.69:1180/app/com.ictrek.image-hub/image-hub.sh',
      '10.10.11.90:19913',
      'pull',
      'docker.io/library/alpine:3',
    ),
    'curl -k -fsSL https://192.168.8.69:1180/app/com.ictrek.image-hub/image-hub.sh | bash -s -- --ih-addr 10.10.11.90:19913 pull docker.io/library/alpine:3',
  );
  assert.ok(
    buildOneLiner('https://h/image-hub.sh', 'a:1', 'push', 'docker.io/x/y:1').includes(
      'push docker.io/x/y:1',
    ),
  );
});

test('buildScriptDownload / buildDownloadAndRun：curl 带 -k，下载后赋权执行', () => {
  assert.equal(
    buildScriptDownload('https://h/image-hub.sh'),
    'curl -k -fsSL https://h/image-hub.sh -o image-hub.sh',
  );
  assert.equal(
    buildDownloadAndRun('https://h/image-hub.sh', '10.0.0.1:19913', 'pull', 'docker.io/x/y:1'),
    'curl -k -fsSL https://h/image-hub.sh -o image-hub.sh && chmod +x image-hub.sh && ./image-hub.sh --ih-addr 10.0.0.1:19913 pull docker.io/x/y:1',
  );
});

test('buildPlainText 使用翻译函数保留步骤标题（命令本体不变）', () => {
  const translate = (key) => key;
  const text = buildPlainText(
    buildPushCommands('10.0.0.1', 19913, 'docker.io/foo/bar', 'tag'),
    translate,
  );
  assert.ok(text.includes('# commands.push.socat'));
  assert.ok(text.includes('docker push'));
});