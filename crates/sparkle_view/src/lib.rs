//! What a map view does besides drawing, shared by sparkle's frontends: the command line
//! and setting a map up from it, the camera and the tiles it covers, loading and
//! processing tiles from local MBTiles and GeoJSON or tile servers (fetched and cached on
//! disk) on worker threads, which loaded tiles to draw, and the motion of wheel zooms and flings.

pub mod args;
pub mod cache;
pub mod camera;
pub mod fetch;
pub mod loader;
pub mod map;
pub mod mbtiles;
pub mod motion;
pub mod remote;
pub mod setup;
