---
title: Documentation does not invent facts
severity: major
paths: ["**/*.md"]
---
Documentation changes must not invent facts. If a doc states a number, guarantee, SLO or architectural claim, it should be traceable to something else in the repository or to a named external source. A plausible figure with no basis is worse than an acknowledged gap (AGENTS.md R4: flag gaps, do not fill them). Watch particularly for latency, throughput and savings figures: a number that is not measured does not ship.
