/// Represents the checker status. Reloads periodically until checking completes.
function Status({done, generated, model, properties, recent_path}) {
    let status = this;

    status.generated = generated;
    status.model = model.replace(/[A-Za-z_]+::/g, '');
    status.progress = 'Done';
    if (!done) {
        status.progress = recent_path.length < 100
            ? recent_path
            : recent_path.substring(0, 99 - 3) + '...';
    }
    status.properties = properties.map((p) => {
        let expectation = p[0];
        let discoveryPath = p[2];
        return {
            expectation,
            name: p[1],
            discoveryPath,
            summary: (() => {
                if (discoveryPath) {
                    switch (expectation) {
                        case 'Always':     return '⚠️ Counterexample found: ';
                        case 'Sometimes':  return '✅ Example found: ';
                        case 'Eventually': return '⚠️ Counterexample found: ';
                        default:
                            throw new Error(`Invalid expectation ${expectation}.`);
                    }
                } else {
                    if (!done) { return '🔎 Searching: ' };
                    switch (expectation) {
                        case 'Always':     return '✅ Safety holds: ';
                        case 'Sometimes':  return '⚠️ Example not found: ';
                        case 'Eventually': return '✅ Liveness holds: ';
                        default:
                            throw new Error(`Invalid expectation ${expectation}.`);
                    }
                }
            })(),
        };
    });
    status.recentPath = recent_path;
}
/// Placeholder status.
Status.LOADING = new Status({
    done: 'loading...',
    generated: 'loading...',
    model: 'loading...',
    properties: [],
    recent_path: 'loading...',
});

/// Represents a model step. Only loads next steps on demand.
function Step({action, outcome, state, fingerprint, prevStep, svg}) {
    let step = this;

    step.action = action || `Init ${i}`;
    step.outcome = outcome;
    step.state = state;
    step.svg = svg;
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
                    svg: nextStep.svg,
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
    step.isIgnored = 'undefined' === typeof step.state;
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
    app.isSameStateAsSelected = (step) => step.state == app.selectedStep().state;
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
