# The #1 Skill for Working with AI Agents — It's Not Coding

> Video script for English oral practice. ~7 min read-aloud, ~1200 words.

---

## [HOOK — 0:00]

Everyone's talking about Prompt Engineering.

There are courses, tutorials, cheat sheets — like it's this brand-new skill you have to learn from scratch.

But here's the thing. After building several system-level projects with AI agents, I've come to a conclusion that might surprise you:

**The most important skill for working with AI is not programming. It's communication.**

And I mean the same communication skill you use with people every single day.

Today I want to share two principles that changed how I work with AI. And the best part? You already know both of them.

---

## [THE PROBLEM — 0:30]

Let me start with a scenario.

You're a developer. Your product manager walks over and says:

> "Hey, can you build that feature?"

That's it. That's the whole requirement.

So you spend the next thirty minutes asking: What feature? Which page? What triggers it? Who's it for? What should it look like when it's done?

We've all been there. Vague requirements waste everyone's time.

Now here's what's interesting — people do the exact same thing with AI.

They open ChatGPT or Claude, type something like "help me refactor this code," and then get frustrated when the result isn't what they wanted.

The AI didn't fail you. The communication did.

---

## [PRINCIPLE 1: BE SPECIFIC — 1:30]

The first principle is simple: **say what you actually mean.**

In project management, we have the 5W1H framework — When, Where, Who, What, Why, How. It's been around for decades. Nothing new.

But look at how powerful it is when applied to AI.

OpenAI recently published a blog post about building skills for AI agents. They showed two versions of the same instruction.

The bad version:

> "Run the mandatory verification stack."

The good version:

> "Run the mandatory verification stack when changes affect runtime code, tests, or build/test behavior."

See the difference? The good version answers **when** to do it and **what scope** it covers. The bad version just says what to do, with no context.

This is exactly how you'd give a clear task to a teammate. You wouldn't just say "run the tests." You'd say "run the integration tests after you change the API layer, and make sure the auth flow still works."

Same logic. Same skill. Different audience.

When I was building tcg-rs — a Rust rewrite of QEMU's binary translation engine — I spent serious time writing a file called `CLAUDE.md`. It's essentially an onboarding document for the AI agent. It tells the agent: what the project does, how to build it, what the architecture looks like, what the coding style is, and what design principles to follow.

You know what that is? That's the same onboarding doc you'd write for a new team member on their first day.

**Writing good instructions for AI is not a new skill. It's the same skill as writing good instructions for people.**

---

## [PRINCIPLE 2: PROGRESSIVE DISCLOSURE — 3:30]

The second principle is something I call progressive disclosure. And it might be even more important than the first one.

Imagine this. You join a new company. Day one, your manager dumps three hundred pages of design docs, historical decisions, incident reports, and tech debt memos on your desk and says: "Read all of this, then come find me."

You'd be overwhelmed. Nobody onboards like that.

The normal way is: start with the big picture. What does the team do? What's the core architecture? What's your piece of the puzzle? Get that down in week one. Then go deeper as needed.

**The same thing applies to AI.**

A lot of people make the mistake of cramming everything into one massive prompt. Ten thousand words of context, thirty rules, five complete files… and the AI's performance actually gets *worse*. Because the key information gets buried in noise.

OpenAI's agent skill system is a great example of how to do this right. They designed a three-layer loading model:

**Layer one — Metadata.** Just the name and a short description. This loads at startup so the agent knows what skills exist.

**Layer two — Full documentation.** This only loads when the agent decides it needs a specific skill.

**Layer three — Scripts and assets.** These only activate during actual execution.

First, let the agent know what's available. Then, tell it how to do it. Finally, give it the tools. Each layer loads on demand. Nothing wasted.

In your day-to-day conversations with AI, the same principle works beautifully.

Instead of writing a three-thousand-word prompt explaining your entire project and then asking for a small code change at the end — try this:

**Step one:** "I need to add a CMOV instruction to the x86 backend. Take a look at how the existing conditional instructions are implemented."

Let the agent read the code and understand the pattern.

**Step two:** "Follow the same pattern as SETcc and Jcc. Use the existing Cond enum for condition codes."

Now the agent has context. Give it the specific task.

**Step three — only if needed:** "Note that CMOV uses opcode 0F 4x and needs the P_EXT prefix flag."

Three turns. Each one builds on the last. Way more effective than one giant wall of text.

---

## [PUTTING IT TOGETHER — 5:30]

These two principles aren't separate tricks. They're two sides of the same coin.

**Clear communication** is about the quality of each interaction — every message should hit the point.

**Progressive disclosure** is about the rhythm across interactions — what to say, and when to say it.

Put them together and you get: **the right information, at the right time, in the right way.**

This works for people. And it works for AI agents. Because at the end of the day, communication is communication.

---

## [CLOSING — 6:15]

Here's what I want you to take away from this.

The 5W1H framework? That's from project management textbooks, decades old.

Progressive disclosure? That's a classic UX design principle. Jakob Nielsen was writing about it in the 1990s.

Task decomposition and clear ownership? That's basic software engineering.

None of this is new. "Prompt Engineering" is just applying timeless communication wisdom to a new kind of audience.

**You don't need to learn a new skill. You need to transfer the skill you already have.**

If you can explain a requirement clearly to a colleague, you can explain it clearly to an agent. If you know how to onboard a new teammate step by step, you know how to feed context to AI progressively.

And here's the flip side — if your AI interactions feel inefficient, maybe it's not the AI. Maybe it's a communication habit worth examining. AI just makes the problem visible.

In the age of AI, technical barriers are falling fast. But one skill is becoming more valuable, not less:

**The ability to say what you mean, clearly.**

That's it. Learn to communicate well. That's the whole secret.

---

*Thanks for watching. If this was helpful, consider subscribing. See you in the next one.*
