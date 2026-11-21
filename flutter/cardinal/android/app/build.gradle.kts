plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// For a test install beside the real app, with its own data:
// `flutter build apk --profile --android-project-arg=cardinalAppIdSuffix=.offlinetest`
// installs as com.cardinalcarto.cardinal.offlinetest. Without it, the build is unchanged.
val appIdSuffix = (project.findProperty("cardinalAppIdSuffix") as String?) ?: ""

android {
    namespace = "com.cardinalcarto.cardinal"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.cardinalcarto.cardinal" + appIdSuffix
        // The launcher label says which install is which.
        manifestPlaceholders["appLabel"] = "cardinal" + appIdSuffix
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName

        // The sparkle native library is only built for 64-bit ABIs. The Flutter
        // Gradle plugin has already set its own ABIs (armeabi-v7a included) by the
        // time this runs, so they are replaced, not added to.
        ndk {
            abiFilters.clear()
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    // NotificationCompat, ServiceCompat and LocationManagerCompat for navigation's service
    // and fixes; the version the app's plugins already bring in.
    implementation("androidx.core:core-ktx:1.17.0")
}
