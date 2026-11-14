package com.cardinalcarto.cardinal

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

class MainActivity : FlutterActivity() {
    private var navigation: NavigationChannels? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        navigation = NavigationChannels(this, flutterEngine.dartExecutor.binaryMessenger)
    }

    // The engine goes with the activity, and the trip with the engine: nothing is left
    // to update the notification, or to end the trip from it.
    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        navigation?.dispose()
        navigation = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        navigation?.onRequestPermissionsResult(requestCode, grantResults)
    }
}
