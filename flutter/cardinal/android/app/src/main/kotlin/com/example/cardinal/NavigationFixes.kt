package com.cardinalcarto.cardinal

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationManager
import android.os.Build
import android.os.Looper
import android.util.Log
import androidx.core.location.LocationListenerCompat
import androidx.core.location.LocationManagerCompat
import androidx.core.location.LocationRequestCompat

/**
 * Navigation's location updates, on `cardinal/navigation/fixes`, from the platform's
 * [LocationManager], never Google Play services' fused location: as precise as the
 * device gives them, every `intervalMs` (the listen argument), with no distance filter.
 *
 * They're navigation's own, rather than geolocator's: geolocator has one position
 * stream per app, which the map's location dot already holds with its own, looser
 * settings, and hands to any later caller whatever settings it asks for.
 *
 * The provider is the one geolocator picks with `forceLocationManager` at its best
 * accuracy: the platform's fused provider on Android 12 and later, else GPS, else the
 * network's. It's picked from those the device has rather than those enabled, so that
 * with location turned off it still registers, and fixes come again once it's turned
 * on.
 *
 * Fixes keep coming with the screen off while [NavigationService], a foreground service
 * of type location, runs: the app's location access is then foreground access.
 *
 * Errors, by code: `off`, location is off (the stream stays open, and fixes come again
 * once it's on); `denied`, no location permission (the stream ends); `failed`, anything
 * else (the stream ends).
 */
class NavigationFixes(private val context: Context) :
    io.flutter.plugin.common.EventChannel.StreamHandler, LocationListenerCompat {
    private val manager = context.getSystemService(Context.LOCATION_SERVICE) as LocationManager
    private var sink: io.flutter.plugin.common.EventChannel.EventSink? = null
    private var provider: String? = null

    override fun onListen(arguments: Any?, events: io.flutter.plugin.common.EventChannel.EventSink) {
        onCancel(null)
        val interval = ((arguments as? Map<*, *>)?.get("intervalMs") as? Number)?.toLong() ?: 1000L
        if (!hasPermission()) {
            events.error("denied", "No location permission", null)
            events.endOfStream()
            return
        }
        val provider = pickProvider()
        if (provider == null) {
            events.error("failed", "No location provider", null)
            events.endOfStream()
            return
        }
        val request = LocationRequestCompat.Builder(interval)
            .setMinUpdateIntervalMillis(interval)
            .setMinUpdateDistanceMeters(0f)
            .setQuality(LocationRequestCompat.QUALITY_HIGH_ACCURACY)
            .build()
        try {
            LocationManagerCompat.requestLocationUpdates(
                manager, provider, request, this, Looper.getMainLooper(),
            )
        } catch (e: SecurityException) {
            events.error("denied", e.message, null)
            events.endOfStream()
            return
        } catch (e: RuntimeException) {
            Log.w(TAG, "Couldn't request location updates", e)
            events.error("failed", e.message, null)
            events.endOfStream()
            return
        }
        sink = events
        this.provider = provider
        if (!LocationManagerCompat.isLocationEnabled(manager) || !manager.isProviderEnabled(provider)) {
            events.error("off", "Location is off", null)
        }
    }

    override fun onCancel(arguments: Any?) {
        if (sink == null) return
        LocationManagerCompat.removeUpdates(manager, this)
        sink = null
        provider = null
    }

    private fun hasPermission() =
        context.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED ||
            context.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) ==
            PackageManager.PERMISSION_GRANTED

    private fun pickProvider(): String? {
        val all = manager.allProviders
        return when {
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
                LocationManager.FUSED_PROVIDER in all -> LocationManager.FUSED_PROVIDER
            LocationManager.GPS_PROVIDER in all -> LocationManager.GPS_PROVIDER
            LocationManager.NETWORK_PROVIDER in all -> LocationManager.NETWORK_PROVIDER
            else -> null
        }
    }

    override fun onLocationChanged(location: Location) {
        sink?.success(toMap(location))
    }

    override fun onProviderDisabled(provider: String) {
        if (provider == this.provider) sink?.error("off", "Location is off", null)
    }


    private companion object {
        const val TAG = "CardinalNavigation"

        /** What Dart's `navLocationFromMap` reads; what wasn't measured is left out. */
        fun toMap(location: Location): Map<String, Any> = buildMap {
            put("latitude", location.latitude)
            put("longitude", location.longitude)
            put("timestamp", location.time)
            if (location.hasAccuracy()) put("accuracy", location.accuracy.toDouble())
            if (location.hasBearing()) put("heading", location.bearing.toDouble())
            if (location.hasSpeed()) put("speed", location.speed.toDouble())
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                if (location.hasBearingAccuracy()) {
                    put("heading_accuracy", location.bearingAccuracyDegrees.toDouble())
                }
                if (location.hasSpeedAccuracy()) {
                    put("speed_accuracy", location.speedAccuracyMetersPerSecond.toDouble())
                }
            }
        }
    }
}
