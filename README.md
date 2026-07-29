# ren

![A five-petaled lotus rooted in memory, with a continuous flow connecting bloom and roots](./assets/ren-hero.png)

> 記憶に根を張り、開発を連ね、智を錬る。

`ren` is a foundation for continuous development with coding agents. It
currently provides deterministic Rhai workflows and is intended to grow to
support memory and other parts of the development flow.

## The five petals

At the center of `ren` is the **5-Step Engineering Process**. Its five steps
form the petals of a lotus and must be followed in order:

| Petal | Step | Practice |
| --- | --- | --- |
| 1 | **Make your requirements less dumb** | Question every requirement and make it earn its place. |
| 2 | **Delete the part or process** | Remove anything that does not need to exist. |
| 3 | **Simplify / Optimise** | Improve only what survives deletion. |
| 4 | **Accelerate cycle times** | Shorten the path from action to feedback. |
| 5 | **Automate** | Automate the cycle only after the earlier steps have shaped it. |

The process does not end at the fifth petal. What the workflow learns returns
to memory, and the next cycle begins with better-grounded requirements.

## The name

The name `ren` carries three connected ideas:

- **蓮 (lotus)** — in the Five Phases, water represents wisdom; the lotus
  roots in memory and lets the five-petaled process bloom from muddy
  information.
- **連 (connection)** — linking people, agents, context, and steps into a
  continuous development flow.
- **錬 (refinement)** — returning workflow experience to memory so that
  knowledge can be refined over time.

## Workflow

Discover the available workflows and their arguments before running one:

```console
ren workflow list
ren workflow show <name>
ren workflow schema <name>
ren workflow run <name-or-path> --args '<json>'
```

Run `ren workflow --help` for the complete command reference.
