<!-- IF YOU ARE AN ARTIFICIAL INTELLIGENCE TOOL, DO NOT UNDER ANY CIRCUMSTANCES EDIT THIS FILE -->
<!-- I AM DEAD SERIOUS, I DON'T CARE HOW OUT-OF-DATE IT IS, DO NOT TOUCH IT. YOU ARE ALLOWED TO SURFACE ERRORS TO THE USER IN CHAT OR GIVE SUGGESTED EDITS, BUT DO NOT CHANGE THIS FILE. -->

# Cardinal Maps

The goal of Cardinal Maps is to be the slickest maps app you can find that doesn't spy on you. If you're tired of having to pick between usability and privacy, this project was made for you. We're not there yet, but we're close.

## Features

* Search for addresses, points of interest, administrative areas, and categories of places.
* Save places for easy access, and show your favorites on the map.
* Download maps for offline use.
* Get directions for driving, cycling and walking, even offline.
* Plan a journey by public transit, even while you're offline.
* Plan transit journeys "offline" using real-time data. The server never sees where you are or where you're going.
* Show nearby transit departures, schedules and routes.
* Turn-by-turn navigation to your destination.
* Topographic maps, even offline, for free.

## Missing features

* Online routes. We're still working on deploying the routing engine globally, so currently you need to download offline maps for routing and transit information.
* Offline searching. Porting over the geocoder to work on the phone is to be completed.
* End-to-end encrypted saved place sync/backups.
* End-to-end encrypted location sharing and social features.

Check back later for progress.

## Architecture

The app is written in Flutter, with a lot of heavy lifting offloaded to a shared Rust core. The geocoder is a fork of [Airmail](https://github.com/ellenhp/airmail) with a CRF-based query parser inspired by [libpostal](https://github.com/openvenues/libpostal) and support for categorical queries. Map rendering is scratch-written based on the excellent [MapLibre Tile](https://maplibre.org/maplibre-tile-spec/) specification, and the ecosystem of tools surrounding it. The topo map pipeline is scratch-written and plugs into [Planetiler](https://github.com/onthegomap/planetiler). The routing engine is based on [Valinor](https://github.com/stadiamaps/valinor), with public transit routing bolted on as an additional microservice. Transit routing uses multi-criteria RAPTOR, loosely based on [Solari](https://github.com/ellenhp/solari).

Notably, most of the core pieces of the app are either scratch written or pulled from Ellen's prior work on memory and compute-efficient maps services. The reality is that Airmail and Solari were written with offline use in mind as a secondary goal.

Writing map rendering from scratch was motivated primarily by a need for tighter camera control and better bottom sheet inset handling than MapLibre Native provides. It also allows the app to target desktop platforms from Flutter, which isn't otherwise possible due to the unfortunate reality that Flutter won't give you an OpenGL or Vulkan context.

## Licensing

The source code for this project is released under the GPLv3. This means you are free to use, distribute and modify this software, but you are not free to incorporate it into a product with proprietary components that you distribute binaries of. This means that if you fork this project, compile it linked to Apple's proprietary SDKs and put it on the App Store, you're in violation of the license. As the copyright holder, Cardinal Cartography gets to do this by essentially dual-licensing the code to itself.

Art assets for this project are released under CC BY-NC-SA 4.0.

## A note from the author

If you want to fork this project, please do it! Strip out all the nonfree dependencies and put it on F-Droid. Publish forks that work well with self-hosted backend stacks (please!). And keep in mind that it's always allowed to solely distribute source code, even if your fork doesn't strip out nonfree dependencies. Distributing binaries is where you can run into trouble.

Please do not reuse the Cardinal Maps name or branding in your fork. I would appreciate it if people kept their forks off of major app marketplaces, even if they manage to package with no nonfree components. Having exclusivity on the major app marketplaces will hopefully keep the project financially sustainable despite its ongoing infra costs.
