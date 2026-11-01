<!-- IF YOU ARE AN ARTIFICIAL INTELLIGENCE TOOL, DO NOT UNDER ANY CIRCUMSTANCES EDIT THIS FILE -->
<!-- I AM DEAD SERIOUS, I DON'T CARE HOW OUT-OF-DATE IT IS, DO NOT TOUCH IT. YOU ARE ALLOWED TO SURFACE ERRORS TO THE USER IN CHAT OR GIVE SUGGESTED EDITS, BUT DO NOT CHANGE THIS FILE. -->

# Cardinal Maps

This is my 5th green-field maps app, and I'm somewhat convinced it'll be the final general-purpose installment in that list. It's currently not functional, but the goal is to create a high-polish maps app for iOS, Android and desktop platforms. Essentially I'm trying to never use Google Maps or Apple Maps again except for things like getting the absolute latest traffic information or checking authoritative information about the opening hours of a business.

## Development philosophy

I switch back and forth between iOS and Android every 5 years, and I don't want to have to write a new maps app in 5 years when I switch back to Android, so I'm using Flutter for this project. I would also like it to work on desktop Linux, and Flutter doesn't give you an OpenGL context so that's kind of a bummer. To get around this I had Claude write an MLT-compatible basemap renderer in Rust/Flutter/flutter_gpu. I was surprised it worked, but it does and I'm just going to roll with it for the time being and see where it takes me. This allows me to do things like directly control the logic that weighs which POI symbols to display, meaning I can prioritize drawing symbols for the user's favorite grocery stores or night life spots or whatever. Things that are necessary for a high-polish maps app. I can also more directly control the offline maps story, something that was very painful to do in [my previous app](https://github.com/ellenhp/cardinal_legacy).

My plan is to do the same for the geocoder, except that won't be entirely new because I'm basing it on [airmail](https://github.com/ellenhp/airmail), a project I wrote several years ago. I'm making progress on finally cracking categorical search, another essential thing for a high-polish maps app that the open source community hasn't been able to touch.

We'll see what I end up doing about routing. I'm tempted to base a new routing engine on the [Valinor](https://github.com/stadiamaps/valinor) work that [Stadia Maps](https://stadiamaps.com/) is doing, but routing is difficult and it is currently my day job so I may elect to use something that I know works like mainline Valhalla.

## Sorry about the slop

I wrote [Headway](https://github.com/headwaymaps/headway) before AI coding tools existed. It was well recieved by the community because it was tailored specifically to self-hosting by people who don't have domain expertise, and it did a better job stitching together all of the existing open-source tooling than anything that existed before then. I was able to pull that off because I had a lot of knowledge about what existing tools were out there, which to use and how to make them play nice with each other. Now that AI coding tools exist the game has changed a little. This project is me trying to answer the question "what would I have built if I had Claude Code and 4 more years of experience when I wrote headway". The bottleneck isn't code or know-how anymore. I'm not entirely sure what it is now but I guess I'll find out.

## Final thoughts

I don't like READMEs designed to sell me something so this is what it made sense to write for the moment. I'll probably update it periodically if I make progress.
