//! The testing service of the workbench: mainThreadTesting.ts (the extension host's side), testServiceImpl.ts
//! (runs by profile), testProfileService.ts and testResultService.ts (the live results, published to the host
//! when they complete).

use std::collections::HashMap;

use exthost::session::ForwardedRequest;
use gpui::Context;
use serde_json::{Value, json};

use super::Workbench;
use crate::commands::Outcome;
use crate::testing::{GROUP_COVERAGE, GROUP_DEBUG, GROUP_RUN, Profile, ResultState, TestCollection, TestResult, default_profile};

/// A `$registerTestController` registration: its label and `TestControllerCapability` bits.
#[derive(Clone, Debug)]
pub struct Controller {
    pub label: String,
    pub capabilities: u64,
}

#[derive(Default)]
pub struct TestingService {
    pub controllers: HashMap<String, Controller>,
    pub collection: TestCollection,
    pub profiles: Vec<Profile>,
    /// Newest first, as testResultService.ts keeps them.
    pub results: Vec<TestResult>,
    subscribed: bool,
    next_run: u64,
}

impl TestingService {
    pub fn result_mut(&mut self, id: &str) -> Option<&mut TestResult> {
        self.results.iter_mut().find(|result| result.id == id)
    }

    /// The state of a test in the newest result that ran it (`testResultService.getStateById`).
    pub fn state_of(&self, id: &str) -> Option<ResultState> {
        self.results.iter().find(|result| result.items.contains_key(id)).map(|result| result.computed_state(id))
    }

    pub fn is_running(&self) -> bool {
        self.results.iter().any(|result| result.completed_at.is_none())
    }

    /// `testResultService.createLiveResult`; testResultService.ts keeps the newest 128 results.
    fn create_result(&mut self, id: Option<String>, request: Value) -> String {
        let id = id.unwrap_or_else(|| {
            self.next_run += 1;
            format!("r{}-{}", std::process::id(), self.next_run)
        });
        self.results.insert(0, TestResult::new(id.clone(), request));
        self.results.truncate(128);
        id
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl Workbench {
    /// MainThreadTesting's methods.
    pub(crate) fn handle_testing_request(&mut self, request: ForwardedRequest, cx: &mut Context<Self>) -> Outcome {
        let args = request.args;
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let text = |index: usize| args.get(index).and_then(Value::as_str).unwrap_or_default().to_owned();
        let testing = &mut self.testing;
        match request.method.as_str() {
            "$registerTestController" => _ = testing.controllers.insert(text(0), Controller { label: text(1), capabilities: arg(2).as_u64().unwrap_or_default() }),
            "$updateController" => {
                if let Some(controller) = testing.controllers.get_mut(&text(0)) {
                    if let Some(label) = arg(1)["label"].as_str() {
                        controller.label = label.to_owned();
                    }
                    if let Some(capabilities) = arg(1)["capabilities"].as_u64() {
                        controller.capabilities = capabilities;
                    }
                }
            }
            "$unregisterTestController" => {
                let id = text(0);
                testing.controllers.remove(&id);
                testing.profiles.retain(|profile| profile.controller_id != id);
                let diff = [json!({ "op": 3, "itemId": id })];
                testing.collection.apply(&diff);
                if testing.subscribed {
                    self.host.send("ExtHostTesting", "$acceptDiff", vec![json!(diff)]);
                }
            }
            "$subscribeToDiffs" => {
                testing.subscribed = true;
                let diff = testing.collection.reviver_diff();
                self.host.send("ExtHostTesting", "$acceptDiff", vec![json!(diff)]);
            }
            "$unsubscribeFromDiffs" => testing.subscribed = false,
            "$publishDiff" => {
                let diff = arg(1).as_array().cloned().unwrap_or_default();
                for root in testing.collection.apply(&diff) {
                    // The explorer shows the roots expanded, which resolves their children (treeProjection.ts `expandElement`).
                    self.host.send("ExtHostTesting", "$expandTest", vec![json!(root), json!(0)]);
                }
                if testing.subscribed {
                    self.host.send("ExtHostTesting", "$acceptDiff", vec![json!(diff)]);
                }
            }
            "$publishTestRunProfile" => {
                let profile = Profile::from_value(&arg(0));
                if testing.controllers.contains_key(&profile.controller_id) {
                    testing.profiles.retain(|existing| (existing.controller_id.as_str(), existing.profile_id) != (profile.controller_id.as_str(), profile.profile_id));
                    testing.profiles.push(profile);
                    self.send_default_profiles();
                }
            }
            "$updateTestRunConfig" => {
                let (controller, id) = (text(0), arg(1).as_i64());
                if let Some(profile) = testing.profiles.iter_mut().find(|profile| profile.controller_id == controller && Some(profile.profile_id) == id) {
                    profile.update(&arg(2));
                }
                self.send_default_profiles();
            }
            "$removeTestProfile" => {
                let (controller, id) = (text(0), arg(1).as_i64());
                testing.profiles.retain(|profile| !(profile.controller_id == controller && Some(profile.profile_id) == id));
                self.send_default_profiles();
            }
            "$runTests" => {
                let id = self.run_resolved_tests(arg(0), cx);
                return Outcome::Ready(Ok(json!(id)));
            }
            "$startedExtensionTestRun" => {
                let request = arg(0);
                let targets = json!({ "group": request["profile"]["group"].as_u64().unwrap_or(GROUP_RUN), "targets": [{ "testIds": request["include"], "controllerId": request["controllerId"], "profileId": request["profile"]["id"] }] });
                testing.create_result(request["id"].as_str().map(str::to_owned), targets);
            }
            "$startedTestRunTask" => {
                if let Some(result) = testing.result_mut(&text(0)) {
                    result.add_task(&arg(1));
                }
            }
            "$finishedTestRunTask" => {
                if let Some(result) = testing.result_mut(&text(0)) {
                    result.mark_task_complete(&text(1));
                }
            }
            "$finishedExtensionTestRun" => self.finish_test_run(&text(0)),
            "$applyTestRunUpdates" => {
                let run = text(1);
                for update in arg(2).as_array().into_iter().flatten() {
                    self.apply_test_run_update(&run, update);
                }
            }
            "$addTestsToRun" => self.apply_test_run_update(&text(1), &json!({ "type": "tests", "tests": arg(2) })),
            "$updateTestStateInRun" => self.apply_test_run_update(&text(0), &json!({ "type": "state", "taskId": arg(1), "testId": arg(2), "state": arg(3), "duration": arg(4) })),
            "$appendTestMessagesInRun" => self.apply_test_run_update(&text(0), &json!({ "type": "messages", "taskId": arg(1), "testId": arg(2), "messages": arg(3) })),
            "$appendOutputToRun" => self.apply_test_run_update(&text(0), &json!({ "type": "output", "taskId": arg(1), "output": arg(2), "location": arg(3), "testId": arg(4) })),
            "$markTestRetired" => {}
            "$appendCoverage" | "$getCoverageDetails" => return Outcome::Ready(Ok(json!([]))),
            _ => {}
        }
        cx.notify();
        Outcome::done()
    }

    /// `TestRunUpdate`s of a run: tests added, a state, messages or output.
    fn apply_test_run_update(&mut self, run: &str, update: &Value) {
        let Some(result) = self.testing.result_mut(run) else { return };
        let text = |key: &str| update[key].as_str().unwrap_or_default();
        match text("type") {
            "tests" => result.add_chain(update["tests"].as_array().map_or(&[], Vec::as_slice)),
            "state" => result.update_state(text("testId"), text("taskId"), ResultState::from_value(&update["state"]), update["duration"].as_f64()),
            "messages" => {
                for message in update["messages"].as_array().into_iter().flatten() {
                    result.append_message(text("testId"), text("taskId"), message.clone());
                }
            }
            "output" => {
                // A string, or a VSBuffer the session turned into its bytes.
                let output = match &update["output"] {
                    Value::String(output) => output.clone(),
                    bytes => String::from_utf8_lossy(&bytes.as_array().into_iter().flatten().filter_map(|byte| byte.as_u64().map(|byte| byte as u8)).collect::<Vec<u8>>()).into_owned(),
                };
                let test = update["testId"].as_str();
                result.append_output(text("taskId"), &output, (!update["location"].is_null()).then(|| update["location"].clone()), test);
                self.test_output_changed(run);
            }
            _ => {}
        }
    }

    /// `markComplete` and the `$publishTestResults` of testResultService.ts's `onResultsChanged`.
    fn finish_test_run(&mut self, run: &str) {
        let Some(result) = self.testing.result_mut(run) else { return };
        if result.completed_at.is_some() {
            return;
        }
        result.mark_complete(now_millis());
        let serialized = result.to_json();
        self.host.send("ExtHostTesting", "$publishTestResults", vec![json!([serialized])]);
        self.test_run_finished(run);
    }

    /// mainThreadTesting.ts `onDidChange` of the profiles: `$setDefaultRunProfiles`.
    fn send_default_profiles(&self) {
        let mut defaults: HashMap<String, Vec<i64>> = HashMap::new();
        for group in [GROUP_RUN, GROUP_DEBUG, GROUP_COVERAGE] {
            for profile in self.testing.profiles.iter().filter(|profile| profile.group & group != 0 && profile.is_default) {
                defaults.entry(profile.controller_id.clone()).or_default().push(profile.profile_id);
            }
        }
        self.host.send("ExtHostTesting", "$setDefaultRunProfiles", vec![json!(defaults)]);
    }

    /// testServiceImpl.ts `runTests`: the tests grouped by the profile of the group that runs each of them.
    pub(crate) fn run_tests(&mut self, test_ids: &[String], group: u64, cx: &mut Context<Self>) -> Option<String> {
        let mut targets: Vec<(String, i64, Vec<String>)> = Vec::new();
        for id in test_ids {
            let Some(test) = self.testing.collection.items.get(id) else { continue };
            let Some(profile) = default_profile(&self.testing.profiles, group, test) else { continue };
            match targets.iter_mut().find(|(controller, profile_id, _)| *controller == profile.controller_id && *profile_id == profile.profile_id) {
                Some((_, _, ids)) => ids.push(id.clone()),
                None => targets.push((profile.controller_id.clone(), profile.profile_id, vec![id.clone()])),
            }
        }
        if targets.is_empty() {
            return None;
        }
        let targets: Vec<Value> = targets.into_iter().map(|(controller, profile, ids)| json!({ "controllerId": controller, "profileId": profile, "testIds": ids })).collect();
        Some(self.run_resolved_tests(json!({ "group": group, "targets": targets }), cx))
    }

    /// `runResolvedTests`: a live result, then `$runControllerTests` for each controller; the result completes
    /// when every controller answers.
    // shortcut: no workspace trust prompt, the port has no restricted mode.
    fn run_resolved_tests(&mut self, request: Value, cx: &mut Context<Self>) -> String {
        let run = self.testing.create_result(None, request.clone());
        let exclude = request["exclude"].as_array().cloned().unwrap_or_default();
        let mut by_controller: HashMap<String, Vec<Value>> = HashMap::new();
        for target in request["targets"].as_array().into_iter().flatten() {
            let controller = target["controllerId"].as_str().unwrap_or_default().to_owned();
            let ids = target["testIds"].as_array().cloned().unwrap_or_default();
            let exclude: Vec<&Value> = exclude.iter().filter(|id| !ids.contains(id)).collect();
            by_controller.entry(controller.clone()).or_default().push(json!({ "runId": run, "excludeExtIds": exclude, "profileId": target["profileId"], "controllerId": controller, "testIds": ids }));
        }
        let replies: Vec<_> = by_controller.into_values().map(|requests| self.host.call("ExtHostTesting", "$runControllerTests", vec![json!(requests)])).collect();
        self.show_test_results_on_start(&run);
        let finished = run.clone();
        cx.spawn(async move |this, cx| {
            let mut errors = Vec::new();
            for reply in replies {
                if let Ok(Ok(answers)) = reply.await {
                    errors.extend(answers.as_array().into_iter().flatten().filter_map(|answer| answer["error"].as_str().map(str::to_owned)));
                }
            }
            let _ = this.update(cx, |workbench, cx| {
                if !errors.is_empty() {
                    workbench.notify_error(format!("An error occurred attempting to run tests: {}", errors.join(" ")));
                }
                workbench.finish_test_run(&finished);
                cx.notify();
            });
        })
        .detach();
        run
    }

    /// `testing.cancelRun`: `$cancelExtensionTestRun` of the running results.
    pub(crate) fn cancel_test_runs(&mut self) {
        for result in self.testing.results.iter().filter(|result| result.completed_at.is_none()) {
            self.host.send("ExtHostTesting", "$cancelExtensionTestRun", vec![json!(result.id), Value::Null]);
        }
    }
}
