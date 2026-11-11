---
name: architecture-designer
description: Guides system design, codebase modularization, tech stack selection, and generates ADRs interactively with developer sign-offs. Use when starting a new project, refactoring a module, or designing system architecture.
---

# Interactive Architecture & System Design Skill

## Goal
Collaboratively architect software systems by gathering constraints, proposing structural options with trade-offs, recording architectural decisions in `docs/adr/`, and producing phased scaffolding.

## Workflow Rules

### Phase 1: Clarification & Constraint Mapping
- Analyze user requirements, existing code, `GLOSSARY.md`, and `docs/adr/`.
- Ask targeted questions regarding scale, latency budgets, persistence, threading, and UI responsiveness.

### Phase 2: Option Matrix & Trade-Off Evaluation
- Propose 2 to 3 distinct architectural options using the standard Option Evaluation Table (`Option`, `Pros`, `Cons`, `Design Considerations`) with a clear recommendation.
- Wait for user confirmation before locking in hard-to-reverse decisions.

### Phase 3: Architecture Decision Record (ADR)
- Record confirmed hard-to-reverse decisions under `docs/adr/NNNN-<decision-slug>.md` and update domain terminology in `GLOSSARY.md`.

### Phase 4: Blueprint & Verification
- Maintain strict separation between `crates/engine` (pure-Rust domain/Git/indexing) and `crates/app` (Slint UI and chart presentation).
