# Support

Angelo is free and open source. One person writes it.

<div class="grid cards" markdown>

- :heart: **[Sponsor on GitHub](https://github.com/sponsors/kvandeh)**

    Sponsorship pays for the work. Any amount helps. It is the most direct way to keep the
    development active.

- :star: **[Star the repository](https://github.com/kvandeh/angelo)**

    It is free. Most people find a tool like this through stars.

</div>

## Other ways to help

**Report a wrong verdict.** The speed features of Angelo must never change a result. If a
score changes between identical runs, or if a mutant survived and a test should have killed
it, please report it. This is the most useful bug report. Include your `angelo.conf`. Also
tell us if `warm_workers = false` changes the result.

**Run Angelo on a real project and tell us the result.** Most measurements come from a
synthetic project and some open-source libraries. Results from different code are more
useful than one more synthetic benchmark. Slow suites and suites with many fixtures are
the most useful.

**Tell us what the report did not tell you.** The survivor list is the product. If it was
confusing or incorrect, or if it did not show what you needed, that is a defect.

## Get help

Open an issue at [github.com/kvandeh/angelo/issues](https://github.com/kvandeh/angelo/issues).

For a bug, give us these details:

- Your `angelo.conf`
- Your Python and pytest versions
- Your operating system
- The last 20 lines that Angelo printed

## Author

**Kieran van der Heijde** writes and owns Angelo. The links in the footer go to LinkedIn,
the repository and sponsorship.
