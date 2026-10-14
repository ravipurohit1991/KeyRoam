import 'package:flutter/material.dart';

import '../app/controller.dart';
import 'common.dart';
import 'dashboard.dart';
import 'onboarding.dart';
import 'unlock_screen.dart';

class KeyRoamApp extends StatefulWidget {
  const KeyRoamApp({super.key, required this.controller});
  final AppController controller;

  @override
  State<KeyRoamApp> createState() => _KeyRoamAppState();
}

class _KeyRoamAppState extends State<KeyRoamApp> with WidgetsBindingObserver {
  final _messenger = GlobalKey<ScaffoldMessengerState>();

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(_showNotice);
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    widget.controller.removeListener(_showNotice);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) widget.controller.appResumed();
  }

  void _showNotice() {
    final n = widget.controller.takeNotice();
    if (n != null) {
      _messenger.currentState?.showSnackBar(SnackBar(content: Text(n)));
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    return MaterialApp(
      title: 'KeyRoam',
      debugShowCheckedModeBanner: false,
      scaffoldMessengerKey: _messenger,
      theme: keyroamTheme(Brightness.light),
      darkTheme: keyroamTheme(Brightness.dark),
      home: Listener(
        // Any pointer activity postpones auto-lock.
        onPointerDown: (_) => c.userActivity(),
        onPointerSignal: (_) => c.userActivity(),
        child: Focus(
          onKeyEvent: (_, _) {
            c.userActivity();
            return KeyEventResult.ignored;
          },
          child: ListenableBuilder(
            listenable: c,
            builder: (context, _) {
              if (c.fatalError != null) {
                return Scaffold(body: Center(child: Text('KeyRoam could not start: ${c.fatalError}')));
              }
              return switch (c.phase) {
                Phase.loading => const Scaffold(body: Center(child: CircularProgressIndicator())),
                Phase.onboarding => OnboardingFlow(controller: c),
                Phase.locked => UnlockScreen(controller: c),
                Phase.unlocked => Dashboard(controller: c),
              };
            },
          ),
        ),
      ),
    );
  }
}
