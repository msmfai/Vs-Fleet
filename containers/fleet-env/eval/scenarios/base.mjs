// Base scenario — an empty /home/coder/project on the default image (§7). The
// proven default every "base*" behaviour runs against. No docker caps, no setup.

/** @type {import("./_contract.mjs").Scenario[]} */
export const scenarios = [
  {
    id: "base",
    title: "Base — empty workspace, default image",
    image: "fleet-env:latest",
    expectBoot: "ok",
  },
];
