import { test } from "node:test";
import assert from "node:assert/strict";
import { readEndpoint, endpointEditor } from "../src/endpoint-editor.ts";
import { readSchedule, saveSchedule, triggerLabel } from "../src/schedules.ts";
import { historyCards, historyOperations } from "../src/history-view.ts";
import { runStatus } from "../src/presentation.ts";

function form(values:Record<string,string>) {
  const data=new FormData();
  for(const [key,value] of Object.entries(values))data.set(key,value);
  return data;
}
test("WebDAV fields preserve the real endpoint contract without JSON or a password",()=>{
  const remote=readEndpoint(form({"source-provider":"webdav","source-root":"https://nas.example/dav/été/","source-credential_ref":"DAV"}),"source");
  assert.deepEqual(remote,{provider:"webdav",root:"https://nas.example/dav/été/",credential_ref:"DAV"});
  const html=endpointEditor("source","",remote,["local","webdav"]);
  assert.match(html,/source-root/);
  assert.doesNotMatch(html,/JSON|type="password"/);
});
test("FTP consent, optional FTPS CA and OneDrive host lists retain their types",()=>{
  assert.deepEqual(readEndpoint(form({"source-provider":"ftps","source-address":"127.0.0.1:21","source-tls_domain":"nas.example","source-username":"backup","source-root":"/backup","source-credential_ref":"FTPS","source-exclusive_writers":"on"}),"source"),{provider:"ftps",address:"127.0.0.1:21",tls_domain:"nas.example",username:"backup",root:"/backup",credential_ref:"FTPS",root_certificate:null,exclusive_writers:true});
  const ftp=readEndpoint(form({"source-provider":"ftp"}),"source");
  assert.equal(ftp?.acknowledge_plaintext,false);
  assert.equal(ftp?.exclusive_writers,false);
  assert.deepEqual(readEndpoint(form({"source-provider":"onedrive","source-download_hosts":"a.example\r\nb.example\n"}),"source")?.download_hosts,["a.example","b.example"]);
});
test("schedule is manual by default; logon and delayed logon have distinct contracts",()=>{
  assert.equal(readSchedule(form({"schedule-kind":"none"})),undefined);
  assert.deepEqual(readSchedule(form({"schedule-kind":"logon"})),{kind:"logon"});
  assert.deepEqual(readSchedule(form({"schedule-kind":"logon_delayed",delay:"90"})),{kind:"logon_delayed",seconds:90});
  assert.deepEqual(readSchedule(form({"schedule-kind":"daily_utc",time:"08:15"})),{kind:"daily_utc",hour:8,minute:15});
  assert.match(triggerLabel({kind:"logon_delayed",seconds:90}),/90 secondes/);
});
test("schedule consent is fresh and an action targets the requested profile",async()=>{
  const calls:unknown[]=[];
  Object.assign(globalThis,{window:{__TAURI_INTERNALS__:{invoke:async(command:string,args:unknown)=>{calls.push({command,args});}}}});
  await assert.rejects(saveSchedule("second-profile",form({"schedule-kind":"interval",seconds:"60"}),true,false),/Autorisation/);
  assert.equal(calls.length,0);
  await saveSchedule("second-profile",form({"schedule-kind":"interval",seconds:"60",writes:"on",deletions:"on"}),false,false);
  assert.deepEqual(calls,[{command:"schedule_save",args:{id:"second-profile",trigger:{kind:"interval",seconds:60},missed:"run_once",approveWrites:true,approveDeletions:false}}]);
  await saveSchedule("second-profile",form({"schedule-kind":"none"}),false,true);
  assert.deepEqual(calls[1],{command:"schedule_remove",args:{id:"second-profile"}});
});
test("history is an escaped table, preserves refusals and never invents success",()=>{
  const html=historyOperations([{operation:{id:"op",path:"<img src=x>",action:"delete",reason:{code:"absent_from_source"},preconditions:[]},outcome:{status:{status:"refused",refusal:{cause:"source_reappeared"}}}}]);
  assert.match(html,/<table>/);
  assert.match(html,/&lt;img src=x&gt;/);
  assert.match(html,/Refusée/);
  assert.doesNotMatch(html,/<img|<pre>/);
  const cards=historyCards([{execution:"run",started:'{"secs":1,"nanos":0}',state:"finished",plan:null,report:{status:"succeeded",deletions_refused:1,totals:{done:2}}}]);
  assert.match(cards,/Miroir incomplet/);
  assert.equal(runStatus({state:"finished",report:{status:"failed",deletions_refused:1}}),"Échec — état partiel");
});
