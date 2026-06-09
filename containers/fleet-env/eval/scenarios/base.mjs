// Base scenario — an empty /home/coder/project on the default image (§7). The
// proven default every "base*" behaviour runs against. No docker caps, no setup.

/** @type {import("./_contract.mjs").Scenario[]} */
export const scenarios = [
  {
    id: "base",
    title: "Base — empty workspace, default image",
    image: "fleet-env:latest",
    expectBoot: "ok",
    rationale: `
WHAT: Boots the unmodified \`fleet-env:latest\` image on an empty
/home/coder/project with no docker caps and no setup() mutations, and asserts the
environment reaches a clean "ok" boot (the bridge comes up and answers query). This
is the zero-variable control: nothing is cloned, no failure is injected, no memory
or network constraint is applied.

WHY THIS IS CORRECT: A freshly built image opened on an empty single-root folder is
the simplest possible activation path for VS Code / code-server — there is no large
working tree to index, no language toolchain to initialise, no extension that needs a
project file before it activates. Under those conditions the editor and the Fleet
bridge are expected to start fully and report a healthy snapshot, so "ok" is the only
correct outcome. Any other result means the baseline platform itself is broken, not
the workload.

WHY IT MATTERS: This is the anchor every "base*" behaviour is layered on and the
reference point for diffing all heavier scenarios (large-repo, mem-capped, language
variants). If a refactor breaks boot here — image entrypoint change, bridge port/
handshake regression, code-server startup flag drift — it must be caught before any
richer scenario, because every other failure becomes ambiguous (workload bug vs.
broken baseline) once the control itself is red. A green "base" tells a future reader
interrogating a break that the platform is sound and the fault lies in the scenario-
specific setup or behaviour, not in the image.`,
  },
];
