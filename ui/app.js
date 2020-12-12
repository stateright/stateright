/// Represents the checker status. Reloads periodically until checking completes.
function Status({discoveries, done, generated, model}) {
    let status = this;

    status.discoveries = discoveries;
    status.done = done;
    status.generated = generated;
    status.model = model
        .replaceAll('stateright::actor::register::', '')
        .replaceAll('stateright::actor::system::', '')
        .replaceAll('stateright::actor::', '')
        .replaceAll('stateright::', '');
}
/// Placeholder status.
Status.LOADING = new Status({
    discoveries: 'loading...',
    done: 'loading...',
    generated: 'loading...',
    model: 'loading...',
});

/// Represents a model step. Only loads next steps on demand.
function Step({action, outcome, state, fingerprint, prevStep}) {
    let step = this;

    step.action = action || `Init ${i}`;
    step.outcome = outcome;
    step.state = state;
    step.fingerprint = fingerprint;
    step.prevStep = prevStep;

    step.path = prevStep ? prevStep.path + '/' + fingerprint : '';
    step.pathSteps = () => (prevStep ? prevStep.pathSteps() : []).concat([step]);
    step.nextSteps = ko.observableArray();
    step.fetchNextSteps = async () => {
        Step._NEXT_STEPS = Step._NEXT_STEPS || {};
        let cached = Step._NEXT_STEPS[step.path];
        if (cached) { return cached; }

        console.log('Fetching next steps.', {path: step.path});
        Step._NEXT_STEPS[step.path] = fetch(`/.states${step.path}`)
            .then(r => r.json())
            .then((nextSteps, err) => {
                if (err) {
                    console.log(err);
                }
                console.log('Response received.', {path: step.path, nextSteps});
                return nextSteps.map((nextStep, i) => new Step({
                    action: nextStep.action || `Init ${i}`,
                    outcome: nextStep.outcome,
                    state: nextStep.state,
                    fingerprint: nextStep.fingerprint,
                    prevStep: step,
                }));
            })
            .catch(err => {
                console.log('Failed. Removing from cache.', {path: step.path, err});
                Step._NEXT_STEPS[step.path] = undefined;
            });
        let nextSteps = await Step._NEXT_STEPS[step.path];
        step.nextSteps(nextSteps);
        return nextSteps;
    };
}
/// Special step that points to the init steps.
Step.PRE_INIT = new Step({
    action: 'Pre-init',
    state: 'No state selected',
    fingerprint: '',
    prevStep: null,
});

/// Manages app state.
function App() {
    let app = this;

    app.selectedStep = ko.observable(Step.PRE_INIT);
    app.isCompact = ko.observable(false);
    app.isCompleteState = ko.observable(false);
    app.isStepNoOp = (step) => step.state == app.selectedStep().state;
    app.status = ko.observable(Status.LOADING);

    window.onhashchange = prepareView;
    window.onhashchange();
    refreshStatus();

    async function refreshStatus() {
        console.log('Refreshing status.');
        let response = await fetch('/.status');
        let json = await response.json();
        console.log(json);
        app.status(new Status(json));
        if (!json.done) {
            setTimeout(refreshStatus, 5000);
        }
    }
    async function prepareView() {
        let hash = window.location.hash;
        console.log('Hash changed. Preparing view.', {hash});

        // Canonicalize, then extract view name and remaining components.
        hash = hash || '#/steps';
        let components = hash.split('/'); // e.g. ['#', 'steps', 4, 2]
        let view = components[1];
        components.shift();
        components.shift();

        switch (view) {
            case 'steps':
                let step = Step.PRE_INIT;
                while (true) {
                    let nextSteps = await step.fetchNextSteps();
                    let nextFingerprint = components.shift();
                    if (!nextFingerprint) { return app.selectedStep(step); }
                    step = nextSteps.find(step => step.fingerprint == nextFingerprint);
                }
            default: throw new Error(`Invalid view: '${view}'`);
        }
    }
}

window.app = new App();
ko.applyBindings(window.app);
