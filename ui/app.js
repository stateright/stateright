/// Represents the checker status. Reloads periodically until checking completes.
function Status({done, state_count, unique_state_count, model, properties, recent_path}) {
    let status = this;

    status.stateCount = state_count.toLocaleString();
    status.uniqueStateCount = unique_state_count.toLocaleString();
    status.model = model.replace(/[0-9A-Za-z_]+::/g, '');
    status.progress = 'Done';
    if (!done) {
        status.progress = (recent_path || '').length < 100
            ? recent_path
            : recent_path.substring(0, 99 - 3) + '...';
    }
    status.properties = properties.map((p) => { return getProperty(p, done) });
    status.recentPath = recent_path;
}
/// Placeholder status.
Status.LOADING = new Status({
    done: 'loading...',
    state_count: 'loading...',
    unique_state_count: 'loading...',
    model: 'loading...',
    properties: [],
    recent_path: 'loading...',
});

function getProperty(p, done) {
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
}

function getPropertyForState(p, path) {
    let expectation = p[0];
    let discoveryPath = p[2];
    if (discoveryPath) {
        const dp = `/${discoveryPath}`
        console.log(dp, path)
        const prefixOrSuffix = dp.indexOf(path) === 0 || path.indexOf(dp) === 0
        if (!prefixOrSuffix) {
            discoveryPath = ""
        }
    }

    const [ icon, summary ] = (() => {
        if (discoveryPath) {
            if (discoveryPath.length + 1 < path.length) {
                // state before discovery
                return ['⬆️', ''];
            } else if (discoveryPath.length + 1 > path.length) {
                // state after discovery
                return ['⬇️', ''];
            } else {
                switch (expectation) {
                    case 'Always': return [ '⚠️',' Counterexample found: ' ];
                    case 'Sometimes':  return [ '✅', ' Example found: ' ];
                    case 'Eventually': return [ '⚠️', ' Counterexample found: ' ];
                    default:
                        throw new Error(`Invalid expectation ${expectation}.`);
                }
            }
        } else {
            switch (expectation) {
                case 'Always':     return [ '✅', ' Safety holds: ' ];
                case 'Sometimes':  return [ '⚠️', ' Example not found: ' ];
                case 'Eventually': return [ '✅', ' Liveness holds: ' ];
                default:
                    throw new Error(`Invalid expectation ${expectation}.`);
            }
        }
    })()

    return {
        expectation,
        name: p[1],
        discoveryPath,
        summary,
        icon,
    };
}


/// Represents a model step. Only loads next steps on demand.
function Step({action, outcome, state, fingerprint, properties, prevStep, svg}) {
    let step = this;

    step.action = action || `Init ${i}`;
    step.outcome = outcome;
    step.state = state;
    step.svg = svg;
    step.fingerprint = fingerprint;
    step.prevStep = prevStep;

    step.path = prevStep ? prevStep.path + '/' + fingerprint : '';

    step.properties = properties.map((p) => { return getPropertyForState(p, step.path) });
    step.icons = step.properties.map((p) => { return p.icon }).join(' ')

    step.pathSteps = () => (prevStep ? prevStep.pathSteps() : []).concat([step]);
    step.nextSteps = ko.observableArray();
    step.computeOffsetTo = (dstStep) => {
        let offset = 0;
        for (let cursor = step; cursor; cursor = cursor.prevStep) {
            if (cursor == dstStep) { return offset; }
            ++offset;
        }
        return null;
    };
    step.computeUriWithOffset = (offset) => {
        return `#/steps${step.path}?offset=${offset}`;
    };
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
                    properties: nextStep.properties,
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
    properties: [],
    prevStep: null,
});

/// Manages app state.
function App() {
    let app = this;

    app.selectedStep = ko.observable(Step.PRE_INIT);
    app.farthestStep = ko.observable(Step.PRE_INIT);
    app.isCompact = ko.observable(false);
    app.isCompleteState = ko.observable(false);
    app.isSameStateAsSelected = (step) => step.state == app.selectedStep().state;
    app.onKeyDown = (data, ev) => {
        switch (ev.keyCode) {
            case 38: // up arrow
            case 75: { // k (vim style)
                let offset = app.farthestStep().computeOffsetTo(app.selectedStep());
                offset = Math.min(offset + 1, app.farthestStep().pathSteps().length - 1);
                window.location = app.farthestStep().computeUriWithOffset(offset);
                break;
            }
            case 40: // down arrow
            case 74: { // k (vim style)
                let offset = app.farthestStep().computeOffsetTo(app.selectedStep());
                offset = Math.max(offset - 1, 0);
                window.location = app.farthestStep().computeUriWithOffset(offset);
                break;
            }
            default:
                return true;
        }
    };
    app.status = ko.observable(Status.LOADING);

    window.onhashchange = prepareView;
    window.onhashchange();
    refreshStatus();

    async function refreshStatus() {
        console.log('Refreshing status.');
        let response = await fetch('/.status');
        let json = await response.json();
        console.log({json});
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

        let [path, queryString] = hash.split('?');
        console.log({path, queryString});

        let components = path.split('/'); // e.g. ['#', 'steps', 4, 2]
        let view = components[1];
        components.shift();
        components.shift();

        switch (view) {
            case 'steps':
                let step = Step.PRE_INIT;
                while (true) {
                    let nextSteps = await step.fetchNextSteps();
                    let nextFingerprint = components.shift();
                    if (!nextFingerprint) {
                        app.selectedStep(step);
                        app.farthestStep(step);
                        break;
                    }
                    step = nextSteps.find(step => step.fingerprint == nextFingerprint);
                }
                break;
            default: throw new Error(`Invalid view: '${view}'`);
        }

        let pairs = (queryString || '').split('&');
        for (let pair of pairs) {
            let [qsKey, qsVal] = pair.split('=');
            console.log({qsKey, qsVal});

            switch (qsKey) {
                case 'offset':
                    for (let offset = parseInt(qsVal); offset > 0; --offset) {
                        app.selectedStep(app.selectedStep().prevStep);
                    }
                    break;
            }
        }
    }
}

window.app = new App();
ko.applyBindings(window.app);
